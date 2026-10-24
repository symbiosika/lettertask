//! Desktop notifications for appointments: at the reminder time set in
//! Outlook ("15 Min. vorher") and when the appointment starts.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Local, Utc};

use super::Service;
use crate::model::{Calendar, Event};

/// How often the upcoming events are fetched again.
const REFRESH_SECS: i64 = 300;
/// How far ahead events are fetched (covers long reminder times).
const LOOKAHEAD_HOURS: i64 = 3;
/// A start notification is still shown this late (app woke up, sleep, …).
const LATE_SECS: i64 = 300;

#[derive(Default)]
pub(crate) struct ReminderState {
    events: Vec<Event>,
    fetched: Option<DateTime<Utc>>,
    /// "<event id>|<start>|pre" / "…|start": shown once each.
    shown: HashSet<String>,
}

/// What to announce now for an event: (key, title).
pub(crate) fn due_notice(ev: &Event, now: DateTime<Utc>) -> Option<(String, String)> {
    if ev.is_all_day || ev.is_cancelled || ev.response_status == "declined" || ev.show_as == "free" {
        return None;
    }
    let start = DateTime::parse_from_rfc3339(&ev.start).ok()?.with_timezone(&Utc);
    let secs = (start - now).num_seconds();
    let subject = if ev.subject.trim().is_empty() { "(Kein Betreff)" } else { ev.subject.trim() };
    if secs <= 0 && secs > -LATE_SECS {
        return Some((format!("{}|{}|start", ev.id, ev.start), format!("Jetzt: {subject}")));
    }
    let lead = ev.reminder_minutes.max(0) * 60;
    if ev.is_reminder_on && lead > 0 && secs > 0 && secs <= lead {
        let mins = (secs + 59) / 60;
        return Some((format!("{}|{}|pre", ev.id, ev.start), format!("In {mins} Min.: {subject}")));
    }
    None
}

fn body(ev: &Event) -> String {
    let at = |s: &str| {
        DateTime::parse_from_rfc3339(s)
            .map(|d| d.with_timezone(&Local).format("%H:%M").to_string())
            .unwrap_or_default()
    };
    let mut b = format!("{} – {}", at(&ev.start), at(&ev.end));
    if !ev.location.trim().is_empty() {
        b.push_str(&format!(" · {}", ev.location.trim()));
    } else if ev.is_online_meeting {
        b.push_str(" · Online-Besprechung");
    }
    b
}

impl Service {
    pub(crate) fn start_event_reminders(self: &Arc<Self>) {
        let svc = Arc::downgrade(self);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(5)).await;
            loop {
                let Some(s) = svc.upgrade() else { return };
                s.reminder_tick().await;
                drop(s);
                tokio::time::sleep(Duration::from_secs(30)).await;
            }
        });
    }

    async fn reminder_tick(&self) {
        let now = Utc::now();
        let stale = self
            .reminders
            .lock()
            .unwrap()
            .fetched
            .is_none_or(|t| (now - t).num_seconds() >= REFRESH_SECS);
        if stale {
            let events = self.upcoming_events(now).await;
            let mut st = self.reminders.lock().unwrap();
            st.fetched = Some(now);
            if let Some(events) = events {
                st.events = events;
            }
            // Forget what lies well in the past.
            let keep: HashSet<String> = st.events.iter().map(|e| format!("{}|{}|", e.id, e.start)).collect();
            st.shown.retain(|k| keep.iter().any(|p| k.starts_with(p.as_str())));
        }
        let notices: Vec<(String, String, String)> = {
            let mut st = self.reminders.lock().unwrap();
            let found: Vec<(String, String, String)> = st
                .events
                .iter()
                .filter_map(|ev| due_notice(ev, now).map(|(k, t)| (k, t, body(ev))))
                .collect();
            found.into_iter().filter(|(k, _, _)| st.shown.insert(k.clone())).collect()
        };
        for (_, title, body) in notices {
            self.host.notify(&title, &body);
        }
    }

    /// Own calendars of all Microsoft 365 accounts (not colleagues' ones);
    /// None when nothing could be read (keeps the last list).
    async fn upcoming_events(&self, now: DateTime<Utc>) -> Option<Vec<Event>> {
        let start = (now - chrono::Duration::seconds(LATE_SECS)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let end = (now + chrono::Duration::hours(LOOKAHEAD_HOURS)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let mut out = Vec::new();
        let mut any = false;
        for acc in self.store.list_accounts().ok()? {
            if crate::imap::is_imap_id(&acc.id) && !self.is_google_account(&acc.id) {
                continue;
            }
            let cals: Vec<Calendar> = match self.store.get_account_json(&acc.id, "calendars").ok().flatten() {
                Some(c) => c,
                None => match self.list_calendars(&acc.id).await {
                    Ok(c) => c,
                    Err(_) => continue,
                },
            };
            let ids: Vec<String> = cals.iter().filter(|c| c.owner.is_none() && c.can_edit).map(|c| c.id.clone()).collect();
            if ids.is_empty() {
                continue;
            }
            match self.list_events(&acc.id, &ids, &start, &end).await {
                Ok(evs) => {
                    any = true;
                    out.extend(evs);
                }
                Err(e) => tracing::debug!("Terminerinnerungen: {e}"),
            }
        }
        any.then_some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(start: DateTime<Utc>, reminder: Option<i64>) -> Event {
        let s = start.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        serde_json::from_value(serde_json::json!({
            "id": "e1", "accountId": "a", "calendarId": "c", "subject": "Jour fixe",
            "start": s, "end": s, "isAllDay": false, "location": "", "organizer": null, "attendees": [],
            "showAs": "busy", "categories": [], "isOrganizer": true, "responseStatus": "organizer",
            "bodyPreview": "", "isOnlineMeeting": false, "onlineMeetingUrl": null, "recurrence": null,
            "seriesMasterId": null, "type": "singleInstance", "isReminderOn": reminder.is_some(),
            "reminderMinutes": reminder.unwrap_or(15), "importance": "normal", "sensitivity": "normal",
            "isCancelled": false, "webLink": null
        }))
        .unwrap()
    }

    #[test]
    fn reminder_before_and_at_start() {
        let now = Utc::now();
        let soon = ev(now + chrono::Duration::minutes(10), Some(15));
        let (k, t) = due_notice(&soon, now).unwrap();
        assert!(k.ends_with("|pre"));
        assert_eq!(t, "In 10 Min.: Jour fixe");
        // outside the reminder window: nothing yet
        assert!(due_notice(&ev(now + chrono::Duration::minutes(30), Some(15)), now).is_none());
        // no Outlook reminder: only at the start
        assert!(due_notice(&ev(now + chrono::Duration::minutes(5), None), now).is_none());
        let (k, t) = due_notice(&ev(now - chrono::Duration::seconds(20), None), now).unwrap();
        assert!(k.ends_with("|start"));
        assert_eq!(t, "Jetzt: Jour fixe");
        // long over
        assert!(due_notice(&ev(now - chrono::Duration::minutes(20), Some(15)), now).is_none());
        // declined / cancelled never
        let mut d = ev(now, Some(15));
        d.response_status = "declined".into();
        assert!(due_notice(&d, now).is_none());
    }
}
