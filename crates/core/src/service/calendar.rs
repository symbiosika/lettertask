use std::sync::Arc;

use futures::{stream, StreamExt};
use serde_json::{json, Value};

use super::Service;
use crate::convert::{self, EVENT_SELECT};
use crate::error::{Error, Result};
use crate::graph::{seg, GraphClient};
use crate::model::*;

const CACHE_KEY: &str = "calendars";
/// Colleagues' calendars added inside the app (list of addresses).
const PEOPLE_KEY: &str = "people_calendars";
/// Calendar id prefix of such a calendar: "user:<address>".
const PEOPLE_PREFIX: &str = "user:";
const PEOPLE_GROUP: &str = "Kalender von Personen";
/// Colours a new person's calendar gets at random (Outlook-like palette).
const PEOPLE_COLORS: &[&str] = &[
    "#e8a33d", "#4aa564", "#3f8fd2", "#c2509b", "#8a63d2", "#d9534f", "#2bb3a3", "#9c7b4f", "#5b6ee1", "#e07b39",
];

/// A colleague's calendar added in the app; `name` is the label shown.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
struct PersonCal {
    #[serde(default)]
    name: String,
    #[serde(default)]
    address: String,
    #[serde(default)]
    color: Option<String>,
}

fn utc_header() -> [(&'static str, String); 1] {
    [("Prefer", "outlook.timezone=\"UTC\"".to_string())]
}

impl Service {
    pub(crate) async fn list_calendars(&self, account_id: &str) -> Result<Vec<Calendar>> {
        if crate::imap::is_imap_id(account_id) {
            return Ok(vec![]);
        }
        let c = self.client(account_id)?;
        match c
            .get_all(
                "/me/calendars?$top=100&$select=id,name,color,hexColor,canEdit,isDefaultCalendar",
                &[],
                500,
            )
            .await
        {
            Ok(list) => {
                let mut cals: Vec<Calendar> = list.iter().map(|v| convert::calendar(v, account_id)).collect();
                self.add_group_calendars(&c, account_id, &mut cals).await;
                self.add_people_calendars(account_id, &mut cals)?;
                // Only calendars of other people carry an owner.
                let me = self
                    .store
                    .list_accounts()?
                    .into_iter()
                    .find(|a| a.id == account_id)
                    .and_then(|a| a.mailboxes.into_iter().next())
                    .map(|m| m.address.to_lowercase())
                    .unwrap_or_default();
                for cal in &mut cals {
                    if cal.owner.as_ref().is_some_and(|o| o.address.to_lowercase() == me) {
                        cal.owner = None;
                    }
                }
                cals.sort_by_key(|c| (!c.is_default, c.group.is_some()));
                self.store.put_account_json(account_id, CACHE_KEY, &cals)?;
                Ok(cals)
            }
            Err(e) if e.is_network() => Ok(self
                .store
                .get_account_json(account_id, CACHE_KEY)?
                .unwrap_or_default()),
            Err(e) => Err(e),
        }
    }

    /// `/me/calendars` only covers the default calendar group; calendars the
    /// user added from other people ("Kalender von Personen") or other groups
    /// are listed per group. Failures here keep the plain list.
    async fn add_group_calendars(&self, c: &Arc<GraphClient>, account_id: &str, cals: &mut Vec<Calendar>) {
        let groups = match c.get_all("/me/calendarGroups?$top=50&$select=id,name", &[], 200).await {
            Ok(g) => g,
            Err(e) => {
                tracing::warn!("calendarGroups: {e}");
                return;
            }
        };
        for g in groups {
            let (gid, gname) = (convert::s(&g, "id"), convert::s(&g, "name"));
            if gid.is_empty() {
                continue;
            }
            let path = format!(
                "/me/calendarGroups/{}/calendars?$top=100&$select=id,name,color,hexColor,canEdit,isDefaultCalendar,owner",
                seg(&gid)
            );
            let list = match c.get_all(&path, &[], 500).await {
                Ok(l) => l,
                Err(e) => {
                    tracing::warn!("calendarGroups/{gname}: {e}");
                    continue;
                }
            };
            for v in list {
                let mut cal = convert::calendar(&v, account_id);
                if let Some(known) = cals.iter_mut().find(|k| k.id == cal.id) {
                    known.group = Some(gname.clone());
                    if known.owner.is_none() {
                        known.owner = cal.owner;
                    }
                } else {
                    cal.group = Some(gname.clone());
                    cal.group_id = Some(gid.clone());
                    cals.push(cal);
                }
            }
        }
        // The default group needs no heading of its own.
        let default_group = cals.iter().find(|k| k.is_default).and_then(|k| k.group.clone());
        for cal in cals.iter_mut() {
            if cal.group.is_some() && cal.group == default_group {
                cal.group = None;
            }
        }
    }

    /// Calendars of colleagues added in the app. Outlook keeps calendars added
    /// "from the directory" partly outside of Graph's calendar groups, so they
    /// would otherwise be missing here although Outlook and Teams show them.
    fn add_people_calendars(&self, account_id: &str, cals: &mut Vec<Calendar>) -> Result<()> {
        let people: Vec<PersonCal> = self.store.get_account_json(account_id, PEOPLE_KEY)?.unwrap_or_default();
        for p in people {
            let addr = p.address.to_lowercase();
            if cals.iter().any(|k| k.owner.as_ref().is_some_and(|o| o.address.to_lowercase() == addr)) {
                continue;
            }
            cals.push(Calendar {
                id: format!("{PEOPLE_PREFIX}{addr}"),
                account_id: account_id.to_string(),
                name: if p.name.is_empty() { p.address.clone() } else { p.name.clone() },
                // Added before colours existed: a fixed pick per address.
                color: p.color.clone().or_else(|| {
                    let h = addr.bytes().fold(0usize, |h, b| h.wrapping_mul(31).wrapping_add(b as usize));
                    Some(PEOPLE_COLORS[h % PEOPLE_COLORS.len()].to_string())
                }),
                can_edit: false,
                is_default: false,
                group: Some(PEOPLE_GROUP.to_string()),
                group_id: None,
                owner: Some(Address { name: p.name.clone(), address: addr }),
            });
        }
        Ok(())
    }

    pub(crate) async fn add_people_calendar(&self, account_id: &str, address: &str, name: &str) -> Result<Vec<Calendar>> {
        let address = address.trim().to_lowercase();
        if crate::imap::is_imap_id(account_id) {
            return Err(Error::invalid("Kalender von Personen gibt es nur bei Microsoft 365-Konten."));
        }
        if !address.contains('@') || address.contains(char::is_whitespace) {
            return Err(Error::invalid("Bitte eine gültige E-Mail-Adresse angeben."));
        }
        let mut people: Vec<PersonCal> = self.store.get_account_json(account_id, PEOPLE_KEY)?.unwrap_or_default();
        if !people.iter().any(|p| p.address.eq_ignore_ascii_case(&address)) {
            use rand::seq::SliceRandom;
            let color = PEOPLE_COLORS.choose(&mut rand::thread_rng()).map(|c| c.to_string());
            people.push(PersonCal { name: name.trim().to_string(), address, color });
            self.store.put_account_json(account_id, PEOPLE_KEY, &people)?;
        }
        self.list_calendars(account_id).await
    }

    /// Changes label and/or colour (#rrggbb) of a person's calendar.
    pub(crate) async fn update_people_calendar(&self, account_id: &str, calendar_id: &str, name: Option<&str>, color: Option<&str>) -> Result<Vec<Calendar>> {
        let address = calendar_id.strip_prefix(PEOPLE_PREFIX).unwrap_or(calendar_id);
        let mut people: Vec<PersonCal> = self.store.get_account_json(account_id, PEOPLE_KEY)?.unwrap_or_default();
        let p = people
            .iter_mut()
            .find(|p| p.address.eq_ignore_ascii_case(address))
            .ok_or_else(|| Error::NotFound("Kalender".into()))?;
        if let Some(n) = name {
            p.name = n.trim().to_string();
        }
        if let Some(c) = color {
            let ok = c.len() == 7 && c.starts_with('#') && c[1..].chars().all(|x| x.is_ascii_hexdigit());
            if !ok {
                return Err(Error::invalid("Ungültige Farbe"));
            }
            p.color = Some(c.to_lowercase());
        }
        self.store.put_account_json(account_id, PEOPLE_KEY, &people)?;
        // The cached list must show the change even offline.
        let mut cals: Vec<Calendar> = self.store.get_account_json(account_id, CACHE_KEY)?.unwrap_or_default();
        cals.retain(|c| c.id != calendar_id);
        self.add_people_calendars(account_id, &mut cals)?;
        self.store.put_account_json(account_id, CACHE_KEY, &cals)?;
        Ok(cals)
    }

    pub(crate) async fn remove_people_calendar(&self, account_id: &str, calendar_id: &str) -> Result<Vec<Calendar>> {
        let address = calendar_id.strip_prefix(PEOPLE_PREFIX).unwrap_or(calendar_id);
        let mut people: Vec<PersonCal> = self.store.get_account_json(account_id, PEOPLE_KEY)?.unwrap_or_default();
        people.retain(|p| !p.address.eq_ignore_ascii_case(address));
        self.store.put_account_json(account_id, PEOPLE_KEY, &people)?;
        self.list_calendars(account_id).await
    }

    /// Events of another person's calendar read through their mailbox; without
    /// read access, their free/busy times (getSchedule) as plain blocks.
    async fn person_events(&self, c: &Arc<GraphClient>, account_id: &str, cid: &str, address: &str, start: &str, end: &str) -> Result<Vec<Event>> {
        let path = format!(
            "/users/{}/calendar/calendarView?startDateTime={}&endDateTime={}&$select={EVENT_SELECT}&$top=250",
            seg(address),
            urlencoding::encode(start),
            urlencoding::encode(end),
        );
        match c.get_all(&path, &utc_header(), 5000).await {
            Ok(list) => return Ok(list.iter().map(|v| convert::event(v, account_id, cid)).collect()),
            Err(e) if e.is_network() => return Err(e),
            Err(e) => tracing::info!("calendarView {address}: {e} – free/busy instead"),
        }
        let naive = |t: &str| t.trim_end_matches('Z').to_string();
        let res = c
            .post(
                "/me/calendar/getSchedule",
                &json!({
                    "schedules": [address],
                    "startTime": {"dateTime": naive(start), "timeZone": "UTC"},
                    "endTime": {"dateTime": naive(end), "timeZone": "UTC"},
                    "availabilityViewInterval": 30,
                }),
            )
            .await?;
        let items = res["value"][0]["scheduleItems"].as_array().cloned().unwrap_or_default();
        Ok(items
            .iter()
            .filter(|i| i["status"].as_str() != Some("free"))
            .map(|i| {
                let status = i["status"].as_str().unwrap_or("busy");
                let subject = i["subject"].as_str().filter(|x| !x.is_empty()).map(str::to_string).unwrap_or_else(|| {
                    match status {
                        "tentative" => "Mit Vorbehalt",
                        "oof" => "Abwesend",
                        "workingElsewhere" => "Woanders tätig",
                        _ => "Gebucht",
                    }
                    .to_string()
                });
                let v = json!({
                    "id": format!("fb:{address}:{}:{}", s_of(&i["start"]), s_of(&i["end"])),
                    "subject": subject,
                    "start": i["start"],
                    "end": i["end"],
                    "showAs": status,
                    "location": {"displayName": i["location"].as_str().unwrap_or("")},
                    "isOrganizer": false,
                    "sensitivity": if i["isPrivate"].as_bool() == Some(true) { "private" } else { "normal" },
                });
                convert::event(&v, account_id, cid)
            })
            .collect())
    }

    /// Last known events of the range from the local store (no Graph round trip).
    pub(crate) fn cached_events(&self, account_id: &str, calendar_ids: &[String], start: &str, end: &str) -> Result<Vec<Event>> {
        let start = convert::normalize_utc(start);
        let end = convert::normalize_utc(end);
        if start.is_empty() || end.is_empty() || start >= end {
            return Err(Error::invalid("Ungültiger Zeitraum"));
        }
        let mut out = Vec::new();
        for cid in calendar_ids {
            out.extend(self.store.cached_events(account_id, cid, &start, &end)?);
        }
        out.sort_by(|a, b| a.start.cmp(&b.start).then_with(|| b.end.cmp(&a.end)));
        Ok(out)
    }

    pub(crate) async fn list_events(
        &self,
        account_id: &str,
        calendar_ids: &[String],
        start: &str,
        end: &str,
    ) -> Result<Vec<Event>> {
        if crate::imap::is_imap_id(account_id) {
            return Ok(vec![]);
        }
        let start = convert::normalize_utc(start);
        let end = convert::normalize_utc(end);
        if start.is_empty() || end.is_empty() || start >= end {
            return Err(Error::invalid("Ungültiger Zeitraum"));
        }
        let c = self.client(account_id)?;
        let known: Vec<Calendar> = self.store.get_account_json(account_id, CACHE_KEY)?.unwrap_or_default();
        let results: Vec<(String, Result<Vec<Value>>)> = stream::iter(calendar_ids.iter().cloned())
            .map(|cid| {
                let c = c.clone();
                let base = match (cid.strip_prefix(PEOPLE_PREFIX), known.iter().find(|k| k.id == cid).and_then(|k| k.group_id.as_deref())) {
                    (Some(addr), _) => format!("/users/{}/calendar", seg(addr)),
                    (None, Some(gid)) => format!("/me/calendarGroups/{}/calendars/{}", seg(gid), seg(&cid)),
                    (None, None) => format!("/me/calendars/{}", seg(&cid)),
                };
                let path = format!(
                    "{base}/calendarView?startDateTime={}&endDateTime={}&$select={EVENT_SELECT}&$top=250",
                    urlencoding::encode(&start),
                    urlencoding::encode(&end),
                );
                async move {
                    let r = c.get_all(&path, &utc_header(), 5000).await;
                    (cid, r)
                }
            })
            .buffer_unordered(4)
            .collect()
            .await;
        let mut out = Vec::new();
        for (cid, r) in results {
            // Another person's calendar that is not readable via our own
            // calendar groups: through their mailbox, else free/busy.
            let owner = cid.strip_prefix(PEOPLE_PREFIX).map(str::to_string).or_else(|| {
                known.iter().find(|k| k.id == cid).and_then(|k| k.owner.as_ref()).map(|o| o.address.clone())
            });
            let r = match (r, owner) {
                (Err(e), Some(addr)) if !e.is_network() => {
                    match self.person_events(&c, account_id, &cid, &addr, &start, &end).await {
                        Ok(events) => Ok(events),
                        Err(e) if e.is_network() => Err(e),
                        Err(e) => {
                            // One colleague's calendar must not hide all others.
                            tracing::warn!("calendar of {addr}: {e}");
                            Ok(Vec::new())
                        }
                    }
                }
                (r, _) => r.map(|list| list.iter().map(|v| convert::event(v, account_id, &cid)).collect()),
            };
            match r {
                Ok(events) => {
                    self.store.replace_events(account_id, &cid, &start, &end, &events)?;
                    out.extend(events);
                }
                Err(e) if e.is_network() => {
                    out.extend(self.store.cached_events(account_id, &cid, &start, &end)?);
                }
                Err(e) => return Err(e),
            }
        }
        out.sort_by(|a, b| a.start.cmp(&b.start).then_with(|| b.end.cmp(&a.end)));
        Ok(out)
    }

    async fn fetch_event(&self, c: &Arc<GraphClient>, account_id: &str, event_id: &str) -> Result<(Value, Event)> {
        // Events of other people's calendars are not reachable via /me/events.
        let cached = self.store.event_calendar(account_id, event_id).ok().flatten();
        if event_id.starts_with("fb:") {
            return Err(Error::invalid("Für diesen Termin sind nur die Frei/Gebucht-Zeiten freigegeben."));
        }
        let group_path = cached.as_deref().and_then(|cid| {
            if let Some(addr) = cid.strip_prefix(PEOPLE_PREFIX) {
                return Some(format!("/users/{}/events/{}", seg(addr), seg(event_id)));
            }
            let known: Vec<Calendar> = self.store.get_account_json(account_id, CACHE_KEY).ok().flatten().unwrap_or_default();
            let gid = known.into_iter().find(|k| k.id == cid)?.group_id?;
            Some(format!("/me/calendarGroups/{}/calendars/{}/events/{}", seg(&gid), seg(cid), seg(event_id)))
        });
        let path = group_path.unwrap_or_else(|| format!("/me/events/{}", seg(event_id)));
        let v = c.get_h(&format!("{path}?$select={EVENT_SELECT},body"), &utc_header()).await?;
        let calendar_id = match cached {
            Some(cid) => cid,
            None => self.calendar_of(account_id, event_id, c).await,
        };
        let ev = convert::event(&v, account_id, &calendar_id);
        Ok((v, ev))
    }

    /// Converts a POST/PATCH response. Those come back in the event's own time
    /// zone; known zones are converted locally, unknown ("Customized Time
    /// Zone") ones are re-read with the UTC preference.
    async fn event_from_response(&self, c: &Arc<GraphClient>, account_id: &str, calendar_id: &str, v: &Value) -> Result<Event> {
        let all_day = v["isAllDay"].as_bool().unwrap_or(false);
        let id = v["id"].as_str().unwrap_or("");
        if !all_day && !id.is_empty() && !(convert::dt_tz_resolvable(&v["start"]) && convert::dt_tz_resolvable(&v["end"])) {
            let fresh = c
                .get_h(&format!("/me/events/{}?$select={EVENT_SELECT}", seg(id)), &utc_header())
                .await?;
            return Ok(convert::event(&fresh, account_id, calendar_id));
        }
        Ok(convert::event(v, account_id, calendar_id))
    }

    /// Calendar that holds an event: from the cache, else via Graph.
    async fn calendar_of(&self, account_id: &str, event_id: &str, c: &Arc<GraphClient>) -> String {
        if let Ok(Some(cid)) = self.store.event_calendar(account_id, event_id) {
            return cid;
        }
        if let Ok(Some(cid)) = self.store.kv_get(&format!("evcal|{account_id}|{event_id}")) {
            return cid;
        }
        if let Ok(v) = c
            .get(&format!("/me/events/{}/calendar?$select=id", seg(event_id)))
            .await
        {
            if let Some(id) = v["id"].as_str() {
                let _ = self.store.kv_set(&format!("evcal|{account_id}|{event_id}"), id);
                return id.to_string();
            }
        }
        String::new()
    }

    pub(crate) async fn get_event(&self, account_id: &str, event_id: &str) -> Result<EventDetail> {
        if crate::imap::is_imap_id(account_id) {
            return Err(Error::invalid("Kalender sind für IMAP-Konten nicht verfügbar."));
        }
        let c = self.client(account_id)?;
        let (v, event) = self.fetch_event(&c, account_id, event_id).await?;
        Ok(EventDetail {
            event,
            body_html: convert::body_html(&v),
        })
    }

    fn validate_event(input: &EventInput) -> Result<()> {
        let parse = |ts: &str| {
            let ts = ts.trim();
            chrono::NaiveDateTime::parse_from_str(ts, "%Y-%m-%dT%H:%M:%S%.f")
                .or_else(|_| chrono::NaiveDateTime::parse_from_str(ts, "%Y-%m-%dT%H:%M"))
                .or_else(|_| chrono::DateTime::parse_from_rfc3339(ts).map(|d| d.naive_utc()))
                .or_else(|_| {
                    chrono::NaiveDate::parse_from_str(ts.get(..10).unwrap_or(ts), "%Y-%m-%d")
                        .map(|d| d.and_time(chrono::NaiveTime::MIN))
                })
                .ok()
        };
        let (Some(start), Some(end)) = (parse(&input.start), parse(&input.end)) else {
            return Err(Error::invalid("Beginn und Ende müssen angegeben werden."));
        };
        if input.is_all_day {
            if end.date() <= start.date() {
                return Err(Error::invalid("Ein ganztägiger Termin muss mindestens einen Tag dauern."));
            }
        } else if end < start {
            return Err(Error::invalid("Das Ende liegt vor dem Beginn."));
        }
        if convert::resolve_tz(&input.time_zone).is_none() {
            return Err(Error::invalid(format!("Unbekannte Zeitzone: {}", input.time_zone)));
        }
        if let Some(r) = &input.recurrence {
            Self::validate_recurrence(r)?;
        }
        Ok(())
    }

    fn validate_recurrence(r: &Recurrence) -> Result<()> {
        const DAYS: [&str; 7] = ["monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday"];
        let needs_days = matches!(r.pattern.as_str(), "weekly" | "relativeMonthly" | "relativeYearly");
        match r.pattern.as_str() {
            "daily" | "weekly" | "absoluteMonthly" | "relativeMonthly" | "absoluteYearly" | "relativeYearly" => {}
            other => return Err(Error::invalid(format!("Unbekanntes Serienmuster: {other}"))),
        }
        if needs_days && r.days_of_week.is_empty() {
            return Err(Error::invalid("Bitte mindestens einen Wochentag für die Serie wählen."));
        }
        if let Some(d) = r.days_of_week.iter().find(|d| !DAYS.contains(&d.to_lowercase().as_str())) {
            return Err(Error::invalid(format!("Ungültiger Wochentag: {d}")));
        }
        if let Some(i) = r.index.as_deref().filter(|i| !i.is_empty()) {
            if !matches!(i, "first" | "second" | "third" | "fourth" | "last") {
                return Err(Error::invalid(format!("Ungültige Woche im Monat: {i}")));
            }
        }
        if r.interval < 1 || r.interval > 999 {
            return Err(Error::invalid("Das Intervall der Serie muss zwischen 1 und 999 liegen."));
        }
        if matches!(r.pattern.as_str(), "absoluteMonthly" | "absoluteYearly") && !(1..=31).contains(&r.day_of_month) {
            return Err(Error::invalid("Der Tag im Monat muss zwischen 1 und 31 liegen."));
        }
        if r.pattern.ends_with("Yearly") && !(1..=12).contains(&r.month) {
            return Err(Error::invalid("Der Monat muss zwischen 1 und 12 liegen."));
        }
        let date = |d: &str| chrono::NaiveDate::parse_from_str(d.trim(), "%Y-%m-%d").ok();
        let Some(start) = date(&r.start_date) else {
            return Err(Error::invalid("Die Serie braucht ein gültiges Startdatum."));
        };
        if let Some(end) = r.end_date.as_deref() {
            match date(end) {
                Some(e) if e >= start => {}
                Some(_) => return Err(Error::invalid("Das Ende der Serie liegt vor ihrem Beginn.")),
                None => return Err(Error::invalid("Ungültiges Enddatum der Serie.")),
            }
        }
        if r.occurrences.is_some_and(|n| !(1..=999).contains(&n)) {
            return Err(Error::invalid("Die Anzahl der Termine muss zwischen 1 und 999 liegen."));
        }
        Ok(())
    }

    pub(crate) async fn create_event(&self, account_id: &str, calendar_id: &str, input: EventInput) -> Result<Event> {
        if crate::imap::is_imap_id(account_id) {
            return Err(Error::invalid("Kalender sind für IMAP-Konten nicht verfügbar."));
        }
        Self::validate_event(&input)?;
        let c = self.client(account_id)?;
        let mut body = convert::event_input_to_graph(&input);
        if input.recurrence.is_none() {
            body.as_object_mut().map(|o| o.remove("recurrence"));
        }
        let created = c
            .request_json(
                reqwest::Method::POST,
                &format!("/me/calendars/{}/events", seg(calendar_id)),
                Some(&body),
                &utc_header(),
            )
            .await?;
        let _ = self
            .store
            .kv_set(&format!("evcal|{account_id}|{}", created["id"].as_str().unwrap_or("")), calendar_id);
        // Graph answers POST in the event's own zone (not the Prefer zone).
        let ev = self.event_from_response(&c, account_id, calendar_id, &created).await?;
        self.emit("calendar-changed", json!({"accountId": account_id}));
        Ok(ev)
    }

    pub(crate) async fn update_event(&self, account_id: &str, event_id: &str, input: EventInput) -> Result<Event> {
        if crate::imap::is_imap_id(account_id) {
            return Err(Error::invalid("Kalender sind für IMAP-Konten nicht verfügbar."));
        }
        Self::validate_event(&input)?;
        let c = self.client(account_id)?;
        let (_, existing) = self.fetch_event(&c, account_id, event_id).await?;
        let mut body = convert::event_input_to_graph(&input);
        if let Some(o) = body.as_object_mut() {
            match existing.kind.as_str() {
                // Occurrences cannot carry a recurrence.
                "occurrence" | "exception" => {
                    o.remove("recurrence");
                }
                // Removing the recurrence of a series is explicit (null); an
                // unchanged recurrence is not re-sent, Exchange would drop all
                // exceptions of the series.
                "seriesMaster" => {
                    if let (Some(new), Some(old)) = (&input.recurrence, &existing.recurrence) {
                        if convert::same_recurrence(new, old) {
                            o.remove("recurrence");
                        }
                    }
                }
                _ if input.recurrence.is_none() => {
                    o.remove("recurrence");
                }
                _ => {}
            }
            if !existing.is_organizer {
                // Attendees may only change their own reminder/categories/showAs.
                o.retain(|k, _| matches!(k.as_str(), "categories" | "isReminderOn" | "reminderMinutesBeforeStart" | "showAs"));
            }
        }
        let updated = c
            .request_json(
                reqwest::Method::PATCH,
                &format!("/me/events/{}", seg(event_id)),
                Some(&body),
                &utc_header(),
            )
            .await?;
        let ev = if updated.is_object() && updated["id"].is_string() {
            self.event_from_response(&c, account_id, &existing.calendar_id, &updated).await?
        } else {
            self.fetch_event(&c, account_id, event_id).await?.1
        };
        self.emit("calendar-changed", json!({"accountId": account_id}));
        Ok(ev)
    }

    pub(crate) async fn delete_event(&self, account_id: &str, event_id: &str) -> Result<()> {
        if crate::imap::is_imap_id(account_id) {
            return Err(Error::invalid("Kalender sind für IMAP-Konten nicht verfügbar."));
        }
        let c = self.client(account_id)?;
        c.delete(&format!("/me/events/{}", seg(event_id))).await?;
        self.store.delete_event(account_id, event_id)?;
        self.emit("calendar-changed", json!({"accountId": account_id}));
        Ok(())
    }

    pub(crate) async fn respond_event(
        &self,
        account_id: &str,
        event_id: &str,
        response: &str,
        comment: &str,
        send_response: bool,
    ) -> Result<()> {
        if crate::imap::is_imap_id(account_id) {
            return Err(Error::invalid("Kalender sind für IMAP-Konten nicht verfügbar."));
        }
        if !matches!(response, "accept" | "tentativelyAccept" | "decline") {
            return Err(Error::invalid("Ungültige Antwort"));
        }
        let c = self.client(account_id)?;
        c.post(
            &format!("/me/events/{}/{response}", seg(event_id)),
            &json!({"comment": comment, "sendResponse": send_response}),
        )
        .await?;
        self.emit("calendar-changed", json!({"accountId": account_id}));
        // The server usually removes the invitation from the inbox.
        if let Some(inbox) = self.store.folder_by_well_known(account_id, "inbox")? {
            let _ = self.sync_folder(account_id, &inbox.id, true).await;
        }
        Ok(())
    }
}

fn s_of(v: &Value) -> String {
    v["dateTime"].as_str().unwrap_or_default().to_string()
}
