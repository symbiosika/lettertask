//! Calendar and contacts of accounts signed in with Google (Google Calendar
//! API and People API). Mail of these accounts runs over IMAP/SMTP; the same
//! sign-in also grants the calendar and contacts scopes.

use std::collections::HashMap;

use reqwest::Method;
use serde_json::{json, Value};

use super::calendar::CACHE_KEY;
use super::Service;
use crate::error::{Error, Result};
use crate::gapi::{self, seg, GoogleApi};
use crate::model::*;

/// Default popup reminder (minutes) per calendar.
const REMINDERS_KEY: &str = "gcal_default_reminders";

fn sync_key(account_id: &str) -> String {
    format!("gcontacts-sync|{account_id}")
}

fn events_path(calendar_id: &str) -> String {
    format!("/calendar/v3/calendars/{}/events", seg(calendar_id))
}

impl Service {
    /// IMAP account signed in with Google (calendar and contacts via Google APIs).
    pub(crate) fn is_google_account(&self, account_id: &str) -> bool {
        crate::imap::is_imap_id(account_id) && self.imap_settings(account_id).is_ok_and(|s| s.is_google())
    }

    /// Points the Google APIs at another server (tests).
    pub fn set_google_api_base(&self, base: &str) {
        let b = base.trim_end_matches('/').to_string();
        *self.google_api.write().unwrap() = (b.clone(), b);
    }

    async fn gapi(&self, account_id: &str) -> Result<GoogleApi> {
        let token = self.google_access_token(account_id).await?;
        let (base, people_base) = self.google_api.read().unwrap().clone();
        Ok(GoogleApi { http: self.http.clone(), base, people_base, token })
    }

    // -------------------------------------------------------------- calendar

    pub(crate) async fn google_list_calendars(&self, account_id: &str) -> Result<Vec<Calendar>> {
        let api = match self.gapi(account_id).await {
            Ok(a) => a,
            Err(e) if e.is_network() => return Ok(self.store.get_account_json(account_id, CACHE_KEY)?.unwrap_or_default()),
            Err(e) => return Err(e),
        };
        match api.get_all("/calendar/v3/users/me/calendarList?maxResults=250", "items", 1000).await {
            Ok(list) => {
                let mut cals: Vec<Calendar> = list.iter().map(|v| gapi::calendar(v, account_id)).collect();
                cals.sort_by_key(|c| (!c.is_default, c.group.is_some()));
                self.apply_calendar_overrides(account_id, &mut cals)?;
                let reminders: HashMap<String, i64> = list
                    .iter()
                    .filter_map(|v| {
                        let m = v["defaultReminders"]
                            .as_array()?
                            .iter()
                            .filter(|r| r["method"].as_str() == Some("popup"))
                            .filter_map(|r| r["minutes"].as_i64())
                            .min()?;
                        Some((v["id"].as_str()?.to_string(), m))
                    })
                    .collect();
                self.store.put_account_json(account_id, REMINDERS_KEY, &reminders)?;
                self.store.put_account_json(account_id, CACHE_KEY, &cals)?;
                Ok(cals)
            }
            Err(e) if e.is_network() => Ok(self.store.get_account_json(account_id, CACHE_KEY)?.unwrap_or_default()),
            Err(e) => Err(e),
        }
    }

    fn default_reminders(&self, account_id: &str) -> HashMap<String, i64> {
        self.store.get_account_json(account_id, REMINDERS_KEY).ok().flatten().unwrap_or_default()
    }

    pub(crate) async fn google_list_events(&self, account_id: &str, calendar_ids: &[String], start: &str, end: &str) -> Result<Vec<Event>> {
        let api = match self.gapi(account_id).await {
            Ok(a) => Some(a),
            Err(e) if e.is_network() => None,
            Err(e) => return Err(e),
        };
        let reminders = self.default_reminders(account_id);
        let mut out = Vec::new();
        for cid in calendar_ids {
            let r = match &api {
                Some(api) => {
                    let path = format!(
                        "{}?singleEvents=true&orderBy=startTime&maxResults=2500&timeMin={}&timeMax={}",
                        events_path(cid),
                        seg(start),
                        seg(end)
                    );
                    api.get_all(&path, "items", 10_000).await
                }
                None => Err(Error::Network("offline".into())),
            };
            match r {
                Ok(items) => {
                    let events: Vec<Event> = items
                        .iter()
                        .filter(|v| v["status"].as_str() != Some("cancelled") && v["eventType"].as_str() != Some("workingLocation"))
                        .map(|v| gapi::event(v, account_id, cid, reminders.get(cid).copied()))
                        .collect();
                    self.store.replace_events(account_id, cid, start, end, &events)?;
                    out.extend(events);
                }
                Err(e) if e.is_network() => out.extend(self.store.cached_events(account_id, cid, start, end)?),
                Err(e) if gapi::is_permission_error(&e) || matches!(e, Error::Auth(_)) => return Err(e),
                // One unreadable calendar (removed, no access) must not hide all others.
                Err(e) => tracing::warn!("Google-Kalender {cid}: {e}"),
            }
        }
        out.sort_by(|a, b| a.start.cmp(&b.start).then_with(|| b.end.cmp(&a.end)));
        Ok(out)
    }

    async fn google_fetch_event(&self, api: &GoogleApi, account_id: &str, event_id: &str) -> Result<(Value, Event)> {
        let (cid, gid) = gapi::split_event_id(event_id)?;
        let v = api.get(&format!("{}/{}", events_path(cid), seg(gid))).await?;
        let ev = gapi::event(&v, account_id, cid, self.default_reminders(account_id).get(cid).copied());
        Ok((v, ev))
    }

    pub(crate) async fn google_get_event(&self, account_id: &str, event_id: &str) -> Result<EventDetail> {
        let api = self.gapi(account_id).await?;
        let (v, event) = self.google_fetch_event(&api, account_id, event_id).await?;
        Ok(EventDetail { event, body_html: gapi::body_html(&v) })
    }

    pub(crate) async fn google_create_event(&self, account_id: &str, calendar_id: &str, input: &EventInput) -> Result<Event> {
        let api = self.gapi(account_id).await?;
        let body = gapi::event_input(input, input.is_online_meeting);
        let v = api
            .request(
                Method::POST,
                &format!("{}?sendUpdates=all&conferenceDataVersion=1", events_path(calendar_id)),
                Some(&body),
            )
            .await?;
        let ev = gapi::event(&v, account_id, calendar_id, self.default_reminders(account_id).get(calendar_id).copied());
        self.emit("calendar-changed", json!({"accountId": account_id}));
        Ok(ev)
    }

    pub(crate) async fn google_update_event(&self, account_id: &str, event_id: &str, input: &EventInput) -> Result<Event> {
        let api = self.gapi(account_id).await?;
        let (_, existing) = self.google_fetch_event(&api, account_id, event_id).await?;
        let (cid, gid) = gapi::split_event_id(event_id)?;
        let mut body = gapi::event_input(input, input.is_online_meeting && !existing.is_online_meeting);
        if let Some(o) = body.as_object_mut() {
            match existing.kind.as_str() {
                // Single occurrences cannot carry a recurrence.
                "occurrence" | "exception" => {
                    o.remove("recurrence");
                }
                // An unchanged rule is not re-sent; a removed one is explicit.
                "seriesMaster" => match (&input.recurrence, &existing.recurrence) {
                    (Some(new), Some(old)) if crate::convert::same_recurrence(new, old) => {
                        o.remove("recurrence");
                    }
                    (None, _) => {
                        o.insert("recurrence".into(), json!([]));
                    }
                    _ => {}
                },
                _ => {}
            }
            if !existing.is_organizer {
                // Attendees may only change their own reminder and free/busy.
                o.retain(|k, _| matches!(k.as_str(), "reminders" | "transparency"));
            }
        }
        let v = api
            .request(
                Method::PATCH,
                &format!("{}/{}?sendUpdates=all&conferenceDataVersion=1", events_path(cid), seg(gid)),
                Some(&body),
            )
            .await?;
        let ev = gapi::event(&v, account_id, cid, self.default_reminders(account_id).get(cid).copied());
        self.emit("calendar-changed", json!({"accountId": account_id}));
        Ok(ev)
    }

    pub(crate) async fn google_delete_event(&self, account_id: &str, event_id: &str) -> Result<()> {
        let api = self.gapi(account_id).await?;
        let (cid, gid) = gapi::split_event_id(event_id)?;
        match api
            .request(Method::DELETE, &format!("{}/{}?sendUpdates=all", events_path(cid), seg(gid)), None)
            .await
        {
            // Already gone (410) or never there: the result is the same.
            Err(e) if !e.is_not_found() && !matches!(e, Error::Graph { status: 410, .. }) => return Err(e),
            _ => {}
        }
        self.store.delete_event(account_id, event_id)?;
        self.emit("calendar-changed", json!({"accountId": account_id}));
        Ok(())
    }

    pub(crate) async fn google_respond_event(&self, account_id: &str, event_id: &str, response: &str, comment: &str, send_response: bool) -> Result<()> {
        let api = self.gapi(account_id).await?;
        let (v, _) = self.google_fetch_event(&api, account_id, event_id).await?;
        let attendees = gapi::respond_attendees(&v, response, comment)?;
        let (cid, gid) = gapi::split_event_id(event_id)?;
        api.request(
            Method::PATCH,
            &format!("{}/{}?sendUpdates={}", events_path(cid), seg(gid), if send_response { "all" } else { "none" }),
            Some(&json!({"attendees": attendees})),
        )
        .await?;
        self.emit("calendar-changed", json!({"accountId": account_id}));
        Ok(())
    }

    // -------------------------------------------------------------- contacts

    /// Incremental sync (sync token) of the account's Google contacts.
    pub(crate) async fn google_sync_contacts(&self, account_id: &str) -> Result<SyncResult> {
        let mut result = SyncResult::default();
        let api = self.gapi(account_id).await?;
        let mut token = self.store.kv_get(&sync_key(account_id))?.filter(|t| !t.is_empty());
        let mut full = token.is_none();
        let mut seen = std::collections::HashSet::new();
        let mut page: Option<String> = None;
        loop {
            let mut path = format!(
                "/v1/people/me/connections?personFields={}&pageSize=1000&requestSyncToken=true",
                gapi::PERSON_FIELDS
            );
            if let Some(t) = &token {
                path.push_str(&format!("&syncToken={}", seg(t)));
            }
            if let Some(p) = &page {
                path.push_str(&format!("&pageToken={}", seg(p)));
            }
            let v = match api.get(&path).await {
                Err(Error::Graph { status: 400 | 410, .. }) if token.is_some() => {
                    // Expired sync token: full resync.
                    self.store.kv_set(&sync_key(account_id), "")?;
                    token = None;
                    page = None;
                    full = true;
                    continue;
                }
                Err(e) if gapi::is_permission_error(&e) => {
                    // Signed in before contacts were part of the sign-in.
                    tracing::info!("Google-Kontakte {account_id}: {e}");
                    return Ok(result);
                }
                other => other?,
            };
            for p in v["connections"].as_array().cloned().unwrap_or_default() {
                let resource = p["resourceName"].as_str().unwrap_or("");
                if resource.is_empty() {
                    continue;
                }
                let id = gapi::contact_id(account_id, resource);
                seen.insert(id.clone());
                if p["metadata"]["deleted"].as_bool() == Some(true) {
                    if self.store.get_contact(&id)?.is_some() {
                        self.store.delete_contact(&id)?;
                        result.removed += 1;
                    }
                    continue;
                }
                let contact = gapi::contact(&p, account_id);
                match self.store.get_contact(&id)? {
                    Some(old) if old == contact => {}
                    Some(_) => result.updated += 1,
                    None => result.added += 1,
                }
                self.store.upsert_contact(&contact)?;
            }
            match v["nextPageToken"].as_str().filter(|t| !t.is_empty()) {
                Some(t) => page = Some(t.to_string()),
                None => {
                    if let Some(t) = v["nextSyncToken"].as_str() {
                        self.store.kv_set(&sync_key(account_id), t)?;
                    }
                    break;
                }
            }
        }
        if full {
            let cached = self.store.list_contacts(None, Some(account_id), Some("google"), 0, i64::MAX)?;
            for stale in cached.items.iter().filter(|c| !seen.contains(&c.id)) {
                self.store.delete_contact(&stale.id)?;
                result.removed += 1;
            }
        }
        if result.added + result.updated + result.removed > 0 {
            self.emit("contacts-changed", json!({}));
        }
        Ok(result)
    }

    pub(crate) async fn google_create_contact(&self, account_id: &str, input: &ContactInput) -> Result<Contact> {
        let api = self.gapi(account_id).await?;
        let v = api
            .request(
                Method::POST,
                &format!("/v1/people:createContact?personFields={}", gapi::PERSON_FIELDS),
                Some(&gapi::person_input(input, None)),
            )
            .await?;
        Ok(gapi::contact(&v, account_id))
    }

    pub(crate) async fn google_update_contact(&self, old: &Contact, input: &ContactInput) -> Result<Contact> {
        let resource = gapi::person_resource(&old.id).ok_or_else(|| Error::invalid("Ungültige Kontakt-ID"))?;
        let api = self.gapi(&old.account_id).await?;
        // Google wants the current etag with every change.
        let cur = api.get(&format!("/v1/{resource}?personFields=metadata")).await?;
        let etag = cur["etag"].as_str().unwrap_or("");
        let v = api
            .request(
                Method::PATCH,
                &format!(
                    "/v1/{resource}:updateContact?updatePersonFields={}&personFields={}",
                    gapi::update_person_fields(),
                    gapi::PERSON_FIELDS
                ),
                Some(&gapi::person_input(input, Some(etag))),
            )
            .await?;
        Ok(gapi::contact(&v, &old.account_id))
    }

    pub(crate) async fn google_delete_contact(&self, old: &Contact) -> Result<()> {
        let resource = gapi::person_resource(&old.id).ok_or_else(|| Error::invalid("Ungültige Kontakt-ID"))?;
        let api = self.gapi(&old.account_id).await?;
        match api.request(Method::DELETE, &format!("/v1/{resource}:deleteContact"), None).await {
            Err(e) if !e.is_not_found() => Err(e),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use axum::extract::{Path, Query, State};
    use axum::http::StatusCode;
    use axum::routing::get;
    use axum::{Json, Router};
    use serde_json::{json, Value};

    use crate::host::NullHost;
    use crate::model::*;
    use crate::tokens::MemoryTokenStore;
    use crate::{Service, ServiceOptions};

    /// Google Calendar / People API double: one primary calendar, one
    /// unreadable one, and an address book.
    #[derive(Default)]
    struct Fake {
        events: Vec<Value>,
        contacts: Vec<Value>,
        next: u32,
        send_updates: Vec<String>,
    }
    type S = Arc<Mutex<Fake>>;
    type Q = Query<HashMap<String, String>>;

    async fn calendar_list() -> Json<Value> {
        Json(json!({"items": [
            {"id": "holidays@group", "summary": "Feiertage", "accessRole": "reader", "backgroundColor": "#16a765"},
            {"id": "me@gmail.com", "summary": "me@gmail.com", "summaryOverride": "Privat", "primary": true, "accessRole": "owner",
             "backgroundColor": "#9fe1e7", "defaultReminders": [{"method": "popup", "minutes": 30}]}
        ]}))
    }

    async fn list_events(State(s): State<S>, Path(cal): Path<String>, Query(q): Q) -> (StatusCode, Json<Value>) {
        if cal != "me@gmail.com" {
            return (StatusCode::NOT_FOUND, Json(json!({"error": {"code": 404, "message": "Not Found"}})));
        }
        assert_eq!(q.get("singleEvents").map(String::as_str), Some("true"));
        let all = s.lock().unwrap().events.clone();
        // Two pages, to exercise paging.
        let (items, next) = match q.get("pageToken") {
            None if all.len() > 1 => (all[..1].to_vec(), Some("p2")),
            None => (all, None),
            Some(_) => (all[1..].to_vec(), None),
        };
        (StatusCode::OK, Json(json!({"items": items, "nextPageToken": next})))
    }

    async fn insert_event(State(s): State<S>, Query(q): Q, Json(mut v): Json<Value>) -> Json<Value> {
        let mut f = s.lock().unwrap();
        f.next += 1;
        v["id"] = json!(format!("ev{}", f.next));
        v["organizer"] = json!({"email": "me@gmail.com", "self": true});
        if v["conferenceData"]["createRequest"].is_object() {
            v["hangoutLink"] = json!("https://meet.google.com/xyz");
        }
        f.send_updates.push(q.get("sendUpdates").cloned().unwrap_or_default());
        f.events.push(v.clone());
        Json(v)
    }

    async fn one_event(
        State(s): State<S>,
        Path((_, id)): Path<(String, String)>,
        Query(q): Q,
        method: axum::http::Method,
        body: Option<Json<Value>>,
    ) -> (StatusCode, Json<Value>) {
        let mut f = s.lock().unwrap();
        let Some(i) = f.events.iter().position(|e| e["id"] == json!(id)) else {
            return (StatusCode::NOT_FOUND, Json(json!({"error": {"code": 404, "message": "Not Found"}})));
        };
        f.send_updates.push(q.get("sendUpdates").cloned().unwrap_or_default());
        match method {
            axum::http::Method::DELETE => {
                f.events.remove(i);
                (StatusCode::NO_CONTENT, Json(Value::Null))
            }
            axum::http::Method::PATCH => {
                for (k, v) in body.unwrap().0.as_object().unwrap() {
                    f.events[i][k] = v.clone();
                }
                (StatusCode::OK, Json(f.events[i].clone()))
            }
            _ => (StatusCode::OK, Json(f.events[i].clone())),
        }
    }

    async fn connections(State(s): State<S>, Query(q): Q) -> Json<Value> {
        let f = s.lock().unwrap();
        match q.get("syncToken").map(String::as_str) {
            Some("t1") => Json(json!({"connections": [{"resourceName": "people/c2", "metadata": {"deleted": true}}], "nextSyncToken": "t2"})),
            _ => Json(json!({"connections": f.contacts, "nextSyncToken": "t1"})),
        }
    }

    async fn create_contact(State(s): State<S>, Json(mut v): Json<Value>) -> Json<Value> {
        v["resourceName"] = json!("people/c9");
        v["etag"] = json!("e9");
        s.lock().unwrap().contacts.push(v.clone());
        Json(v)
    }

    async fn person(State(s): State<S>, Path(rest): Path<String>, method: axum::http::Method, body: Option<Json<Value>>) -> (StatusCode, Json<Value>) {
        let mut f = s.lock().unwrap();
        let (id, action) = rest.split_once(':').unwrap_or((&rest, ""));
        let res = format!("people/{id}");
        let Some(i) = f.contacts.iter().position(|c| c["resourceName"] == json!(res)) else {
            return (StatusCode::NOT_FOUND, Json(json!({"error": {"code": 404, "message": "Not Found"}})));
        };
        match (method, action) {
            (axum::http::Method::PATCH, "updateContact") => {
                let mut v = body.unwrap().0;
                assert_eq!(v["etag"], f.contacts[i]["etag"], "update needs the current etag");
                v["resourceName"] = json!(res);
                v["etag"] = json!("e10");
                f.contacts[i] = v.clone();
                (StatusCode::OK, Json(v))
            }
            (axum::http::Method::DELETE, "deleteContact") => {
                f.contacts.remove(i);
                (StatusCode::OK, Json(json!({})))
            }
            _ => (StatusCode::OK, Json(f.contacts[i].clone())),
        }
    }

    async fn setup() -> (Arc<Service>, S, tempfile::TempDir, String) {
        let state: S = Default::default();
        state.lock().unwrap().contacts = vec![
            json!({"resourceName": "people/c1", "etag": "e1", "names": [{"displayName": "Anna A", "givenName": "Anna", "familyName": "A"}],
                   "emailAddresses": [{"value": "anna@x.de"}]}),
            json!({"resourceName": "people/c2", "etag": "e2", "names": [{"displayName": "Bert B"}], "emailAddresses": [{"value": "bert@x.de"}]}),
        ];
        let app = Router::new()
            .route("/calendar/v3/users/me/calendarList", get(calendar_list))
            .route("/calendar/v3/calendars/{cal}/events", get(list_events).post(insert_event))
            .route("/calendar/v3/calendars/{cal}/events/{id}", get(one_event).patch(one_event).delete(one_event))
            .route("/v1/people/me/connections", get(connections))
            .route("/v1/people:createContact", axum::routing::post(create_contact))
            .route("/v1/people/{rest}", get(person).patch(person).delete(person))
            .with_state(state.clone());
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });

        let dir = tempfile::tempdir().unwrap();
        let svc = Service::new(
            ServiceOptions {
                data_dir: dir.path().to_path_buf(),
                graph_base: "http://127.0.0.1:9".into(),
                authority: "http://127.0.0.1:9".into(),
                demo: false,
                token_store: Arc::new(MemoryTokenStore::default()),
            },
            Arc::new(NullHost::default()),
        )
        .unwrap();
        svc.set_google_api_base(&base);
        let id = crate::imap::account_id_for("me@gmail.com");
        svc.store.upsert_account(&id, "me@gmail.com", "Me").unwrap();
        let server = |h: &str| json!({"host": h, "port": 993, "security": "ssl", "username": "me@gmail.com"});
        let settings = json!({"email": "me@gmail.com", "imap": server("imap.gmail.com"), "smtp": server("smtp.gmail.com"), "auth": "google"});
        svc.store.put_account_json(&id, "imap", &settings).unwrap();
        svc.google_tokens.lock().unwrap().insert(
            id.clone(),
            crate::auth::TokenSet {
                access_token: "at".into(),
                refresh_token: "rt".into(),
                expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
            },
        );
        (svc, state, dir, id)
    }

    fn input(subject: &str) -> EventInput {
        serde_json::from_value(json!({
            "subject": subject, "start": "2026-10-26T10:00:00", "end": "2026-10-26T11:00:00",
            "timeZone": "Europe/Berlin", "isReminderOn": true, "reminderMinutes": 10,
            "attendees": [{"address": "anna@x.de"}], "isOnlineMeeting": true
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn calendar_round_trip() {
        let (svc, state, _dir, acc) = setup().await;
        let cals = svc.list_calendars(&acc).await.unwrap();
        assert_eq!(cals[0].id, "me@gmail.com", "primary first");
        assert!(cals[0].is_default && cals[0].can_edit);
        assert_eq!(cals[0].name, "Privat");
        assert_eq!(cals[1].group.as_deref(), Some("Weitere Kalender"));
        assert!(!cals[1].can_edit);

        let created = svc.create_event(&acc, "me@gmail.com", input("Planung")).await.unwrap();
        assert_eq!(created.id, "me@gmail.com|ev1");
        assert_eq!(created.start, "2026-10-26T09:00:00Z");
        assert_eq!(created.online_meeting_url.as_deref(), Some("https://meet.google.com/xyz"));
        svc.create_event(&acc, "me@gmail.com", input("Zweiter")).await.unwrap();

        // Both calendars: the unreadable one is skipped, paging is followed.
        let ids = vec!["me@gmail.com".to_string(), "holidays@group".to_string()];
        let evs = svc.list_events(&acc, &ids, "2026-10-26T00:00:00Z", "2026-10-27T00:00:00Z").await.unwrap();
        assert_eq!(evs.iter().map(|e| e.subject.as_str()).collect::<Vec<_>>(), ["Planung", "Zweiter"]);
        // … and is in the cache for the next start.
        let cached = svc.cached_events(&acc, &ids, "2026-10-26T00:00:00Z", "2026-10-27T00:00:00Z").unwrap();
        assert_eq!(cached.len(), 2);

        let mut changed = input("Planung neu");
        changed.is_online_meeting = true;
        let up = svc.update_event(&acc, &created.id, changed).await.unwrap();
        assert_eq!(up.subject, "Planung neu");
        let detail = svc.get_event(&acc, &created.id).await.unwrap();
        assert_eq!(detail.event.subject, "Planung neu");

        // An invitation from someone else: answer it.
        state.lock().unwrap().events.push(json!({
            "id": "inv", "summary": "Einladung", "organizer": {"email": "boss@x.de"},
            "start": {"dateTime": "2026-10-26T12:00:00Z"}, "end": {"dateTime": "2026-10-26T13:00:00Z"},
            "attendees": [{"email": "boss@x.de", "organizer": true, "responseStatus": "accepted"},
                          {"email": "me@gmail.com", "self": true, "responseStatus": "needsAction"}]
        }));
        svc.respond_event(&acc, "me@gmail.com|inv", "accept", "", false, None).await.unwrap();
        let inv = svc.get_event(&acc, "me@gmail.com|inv").await.unwrap().event;
        assert_eq!(inv.response_status, "accepted");
        assert!(!inv.is_organizer);

        svc.delete_event(&acc, &created.id).await.unwrap();
        assert!(svc.get_event(&acc, &created.id).await.unwrap_err().is_not_found());
        let f = state.lock().unwrap();
        let sent: Vec<&str> = f.send_updates.iter().map(String::as_str).filter(|s| !s.is_empty()).collect();
        assert_eq!(sent, ["all", "all", "all", "none", "all"], "invitations go out, a silent answer does not");
    }

    #[tokio::test]
    async fn contacts_round_trip() {
        let (svc, state, _dir, acc) = setup().await;
        let r = svc.sync_contacts(&acc).await.unwrap();
        assert_eq!(r.added, 2);
        let page = svc.list_contacts(None, Some(&acc), Some("google"), 0, 100).unwrap();
        assert_eq!(page.total, 2);
        // Incremental: Bert was deleted on the server.
        let r = svc.sync_contacts(&acc).await.unwrap();
        assert_eq!(r.removed, 1);

        let input: ContactInput = serde_json::from_value(json!({"givenName": "Carla", "surname": "C", "emails": ["carla@x.de"]})).unwrap();
        let c = svc.create_contact(&acc, input.clone()).await.unwrap();
        assert_eq!(c.source, "google");
        assert_eq!(c.id, format!("{acc}|people/c9"));
        let mut changed = input;
        changed.company = "ACME".into();
        let c = svc.update_contact(&c.id, changed).await.unwrap();
        assert_eq!(c.company, "ACME");
        svc.delete_contact(&c.id).await.unwrap();
        assert!(!state.lock().unwrap().contacts.iter().any(|p| p["resourceName"] == "people/c9"));
        assert!(svc.get_contact(&c.id).is_err());
    }
}
