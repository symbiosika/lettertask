use std::sync::Arc;

use futures::{stream, StreamExt};
use serde_json::{json, Value};

use super::Service;
use crate::convert::{self, EVENT_SELECT};
use crate::error::{Error, Result};
use crate::graph::{seg, GraphClient};
use crate::model::*;

const CACHE_KEY: &str = "calendars";

fn utc_header() -> [(&'static str, String); 1] {
    [("Prefer", "outlook.timezone=\"UTC\"".to_string())]
}

impl Service {
    pub(crate) async fn list_calendars(&self, account_id: &str) -> Result<Vec<Calendar>> {
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
                cals.sort_by_key(|c| !c.is_default);
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

    pub(crate) async fn list_events(
        &self,
        account_id: &str,
        calendar_ids: &[String],
        start: &str,
        end: &str,
    ) -> Result<Vec<Event>> {
        let start = convert::normalize_utc(start);
        let end = convert::normalize_utc(end);
        if start.is_empty() || end.is_empty() || start >= end {
            return Err(Error::invalid("Ungültiger Zeitraum"));
        }
        let c = self.client(account_id)?;
        let results: Vec<(String, Result<Vec<Value>>)> = stream::iter(calendar_ids.iter().cloned())
            .map(|cid| {
                let c = c.clone();
                let path = format!(
                    "/me/calendars/{}/calendarView?startDateTime={}&endDateTime={}&$select={EVENT_SELECT}&$top=250",
                    seg(&cid),
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
            match r {
                Ok(list) => {
                    let events: Vec<Event> = list.iter().map(|v| convert::event(v, account_id, &cid)).collect();
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
        let v = c
            .get_h(
                &format!("/me/events/{}?$select={EVENT_SELECT},body", seg(event_id)),
                &utc_header(),
            )
            .await?;
        let calendar_id = self.calendar_of(account_id, event_id, c).await;
        let ev = convert::event(&v, account_id, &calendar_id);
        Ok((v, ev))
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
        let c = self.client(account_id)?;
        let (v, event) = self.fetch_event(&c, account_id, event_id).await?;
        Ok(EventDetail {
            event,
            body_html: convert::body_html(&v),
        })
    }

    fn validate_event(input: &EventInput) -> Result<()> {
        if input.start.len() < 10 || input.end.len() < 10 {
            return Err(Error::invalid("Beginn und Ende müssen angegeben werden."));
        }
        if input.end < input.start {
            return Err(Error::invalid("Das Ende liegt vor dem Beginn."));
        }
        Ok(())
    }

    pub(crate) async fn create_event(&self, account_id: &str, calendar_id: &str, input: EventInput) -> Result<Event> {
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
        let ev = convert::event(&created, account_id, calendar_id);
        let _ = self
            .store
            .kv_set(&format!("evcal|{account_id}|{}", ev.id), calendar_id);
        self.emit("calendar-changed", json!({"accountId": account_id}));
        Ok(ev)
    }

    pub(crate) async fn update_event(&self, account_id: &str, event_id: &str, input: EventInput) -> Result<Event> {
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
                // Removing the recurrence of a series is explicit (null).
                "seriesMaster" => {}
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
        let ev = if updated.is_object() {
            convert::event(&updated, account_id, &existing.calendar_id)
        } else {
            self.fetch_event(&c, account_id, event_id).await?.1
        };
        self.emit("calendar-changed", json!({"accountId": account_id}));
        Ok(ev)
    }

    pub(crate) async fn delete_event(&self, account_id: &str, event_id: &str) -> Result<()> {
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
