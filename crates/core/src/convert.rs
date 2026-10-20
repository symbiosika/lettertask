//! Mapping between Microsoft Graph JSON and the UI model.

use chrono::{DateTime, NaiveDateTime, SecondsFormat, TimeZone, Utc};
use serde_json::{json, Value};

use crate::model::*;

/// `$select` used for message lists and delta queries.
pub const MESSAGE_SUMMARY_SELECT: &str = "id,conversationId,subject,from,toRecipients,receivedDateTime,isRead,isDraft,importance,flag,hasAttachments,categories,bodyPreview,parentFolderId";

pub const MESSAGE_DETAIL_SELECT: &str = "id,conversationId,subject,from,toRecipients,ccRecipients,bccRecipients,replyTo,receivedDateTime,sentDateTime,isRead,isDraft,importance,flag,hasAttachments,categories,bodyPreview,parentFolderId,body,internetMessageId,webLink,isReadReceiptRequested";

pub const EVENT_SELECT: &str = "id,subject,start,end,isAllDay,location,organizer,attendees,showAs,categories,isOrganizer,responseStatus,bodyPreview,isOnlineMeeting,onlineMeeting,recurrence,seriesMasterId,type,isReminderOn,reminderMinutesBeforeStart,importance,sensitivity,isCancelled,webLink";

fn s(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

fn opt_s(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .filter(|x| !x.is_empty())
        .map(str::to_string)
}

pub fn address(v: &Value) -> Option<Address> {
    let ea = v.get("emailAddress")?;
    Some(Address {
        name: s(ea, "name"),
        address: s(ea, "address"),
    })
}

pub fn addresses(v: &Value, key: &str) -> Vec<Address> {
    v.get(key)
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(address).collect())
        .unwrap_or_default()
}

pub fn recipients_json(list: &[Address]) -> Value {
    Value::Array(
        list.iter()
            .filter(|a| !a.address.trim().is_empty())
            .map(|a| {
                let name = if a.name.trim().is_empty() {
                    a.address.trim()
                } else {
                    a.name.trim()
                };
                json!({"emailAddress": {"name": name, "address": a.address.trim()}})
            })
            .collect(),
    )
}

/// Normalises Graph timestamps (`2026-10-15T14:40:00Z`, with fractions, or
/// without zone) to RFC3339 UTC with `Z`.
pub fn normalize_utc(ts: &str) -> String {
    if ts.is_empty() {
        return String::new();
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(ts) {
        return dt.with_timezone(&Utc).to_rfc3339_opts(SecondsFormat::Secs, true);
    }
    if let Ok(n) = NaiveDateTime::parse_from_str(ts, "%Y-%m-%dT%H:%M:%S%.f") {
        return Utc.from_utc_datetime(&n).to_rfc3339_opts(SecondsFormat::Secs, true);
    }
    ts.to_string()
}

/// Graph `dateTimeTimeZone` → UTC string. We always request
/// `Prefer: outlook.timezone="UTC"`, so the value is UTC wall time.
fn dt_tz(v: &Value) -> String {
    let dt = s(v, "dateTime");
    let tz = s(v, "timeZone");
    if tz.is_empty() || tz.eq_ignore_ascii_case("UTC") || tz.eq_ignore_ascii_case("Etc/GMT") {
        normalize_utc(&dt)
    } else {
        // Unexpected zone; keep wall time as UTC rather than failing.
        normalize_utc(&dt)
    }
}

pub fn message_summary(v: &Value, mailbox_id: &str, fallback_folder: &str) -> MessageSummary {
    MessageSummary {
        id: s(v, "id"),
        mailbox_id: mailbox_id.to_string(),
        folder_id: opt_s(v, "parentFolderId").unwrap_or_else(|| fallback_folder.to_string()),
        conversation_id: s(v, "conversationId"),
        subject: s(v, "subject"),
        from: v.get("from").and_then(address),
        to: addresses(v, "toRecipients"),
        received_at: normalize_utc(&s(v, "receivedDateTime")),
        is_read: v["isRead"].as_bool().unwrap_or(true),
        is_draft: v["isDraft"].as_bool().unwrap_or(false),
        importance: opt_s(v, "importance").unwrap_or_else(|| "normal".into()),
        flag: v["flag"]["flagStatus"]
            .as_str()
            .unwrap_or("notFlagged")
            .to_string(),
        has_attachments: v["hasAttachments"].as_bool().unwrap_or(false),
        categories: string_list(v, "categories"),
        preview: s(v, "bodyPreview"),
        due_at: v["flag"]
            .get("dueDateTime")
            .filter(|d| !s(d, "dateTime").is_empty())
            .map(dt_tz),
        recurrence: recurrence_of(&string_list(v, "categories")),
    }
}

pub fn string_list(v: &Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

pub fn attachment(v: &Value) -> Attachment {
    Attachment {
        id: s(v, "id"),
        name: s(v, "name"),
        content_type: opt_s(v, "contentType").unwrap_or_else(|| "application/octet-stream".into()),
        size: v["size"].as_i64().unwrap_or(0),
        is_inline: v["isInline"].as_bool().unwrap_or(false),
        content_id: opt_s(v, "contentId"),
    }
}

/// Body as HTML. Plain text is escaped and wrapped.
pub fn body_html(v: &Value) -> String {
    let body = &v["body"];
    let content = s(body, "content");
    if s(body, "contentType").eq_ignore_ascii_case("text") {
        text_to_html(&content)
    } else {
        content
    }
}

pub fn text_to_html(text: &str) -> String {
    format!(
        "<pre style=\"white-space:pre-wrap;font-family:inherit;margin:0\">{}</pre>",
        html_escape::encode_text(text)
    )
}

pub fn folder(v: &Value, mailbox_id: &str, root_id: Option<&str>, well_known: Option<String>) -> Folder {
    let parent = opt_s(v, "parentFolderId").filter(|p| Some(p.as_str()) != root_id);
    Folder {
        id: s(v, "id"),
        mailbox_id: mailbox_id.to_string(),
        parent_id: parent,
        display_name: s(v, "displayName"),
        well_known,
        unread_count: v["unreadItemCount"].as_i64().unwrap_or(0),
        total_count: v["totalItemCount"].as_i64().unwrap_or(0),
        child_folder_count: v["childFolderCount"].as_i64().unwrap_or(0),
    }
}

pub fn category(v: &Value) -> Category {
    Category {
        id: s(v, "id"),
        display_name: s(v, "displayName"),
        color: opt_s(v, "color").unwrap_or_else(|| "none".into()),
    }
}

pub fn calendar(v: &Value, account_id: &str) -> Calendar {
    let hex = opt_s(v, "hexColor");
    Calendar {
        id: s(v, "id"),
        account_id: account_id.to_string(),
        name: s(v, "name"),
        color: hex.or_else(|| calendar_color_hex(&s(v, "color")).map(str::to_string)),
        can_edit: v["canEdit"].as_bool().unwrap_or(true),
        is_default: v["isDefaultCalendar"].as_bool().unwrap_or(false),
    }
}

/// Graph `calendarColor` enum → hex (Outlook palette).
pub fn calendar_color_hex(name: &str) -> Option<&'static str> {
    Some(match name {
        "lightBlue" => "#a6d1f5",
        "lightGreen" => "#87d28e",
        "lightOrange" => "#fcab73",
        "lightGray" => "#c0c0c0",
        "lightYellow" => "#f4d07a",
        "lightTeal" => "#79d7ce",
        "lightPink" => "#ee9ab5",
        "lightBrown" => "#cba287",
        "lightRed" => "#f19b9b",
        "maxColor" | "auto" | "" => return None,
        _ => return None,
    })
}

fn recurrence_from_graph(v: &Value) -> Option<Recurrence> {
    if v.is_null() {
        return None;
    }
    let p = &v["pattern"];
    let r = &v["range"];
    let range_type = s(r, "type");
    Some(Recurrence {
        pattern: s(p, "type"),
        interval: p["interval"].as_i64().unwrap_or(1),
        days_of_week: string_list(p, "daysOfWeek"),
        day_of_month: p["dayOfMonth"].as_i64().unwrap_or(0),
        month: p["month"].as_i64().unwrap_or(0),
        start_date: s(r, "startDate"),
        end_date: if range_type == "endDate" {
            opt_s(r, "endDate").filter(|d| d != "0001-01-01")
        } else {
            None
        },
        occurrences: if range_type == "numbered" {
            r["numberOfOccurrences"].as_i64()
        } else {
            None
        },
    })
}

pub fn recurrence_to_graph(r: &Recurrence, time_zone: &str) -> Value {
    let mut pattern = json!({
        "type": r.pattern,
        "interval": r.interval.max(1),
    });
    match r.pattern.as_str() {
        "weekly" => {
            pattern["daysOfWeek"] = json!(r.days_of_week);
            pattern["firstDayOfWeek"] = json!("monday");
        }
        "absoluteMonthly" => pattern["dayOfMonth"] = json!(r.day_of_month.max(1)),
        "absoluteYearly" => {
            pattern["dayOfMonth"] = json!(r.day_of_month.max(1));
            pattern["month"] = json!(r.month.clamp(1, 12));
        }
        _ => {}
    }
    let range = if let Some(end) = &r.end_date {
        json!({"type": "endDate", "startDate": r.start_date, "endDate": end, "recurrenceTimeZone": time_zone})
    } else if let Some(n) = r.occurrences {
        json!({"type": "numbered", "startDate": r.start_date, "numberOfOccurrences": n, "recurrenceTimeZone": time_zone})
    } else {
        json!({"type": "noEnd", "startDate": r.start_date, "recurrenceTimeZone": time_zone})
    };
    json!({"pattern": pattern, "range": range})
}

pub fn event(v: &Value, account_id: &str, calendar_id: &str) -> Event {
    let attendees = v
        .get("attendees")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|x| Attendee {
                    name: s(&x["emailAddress"], "name"),
                    address: s(&x["emailAddress"], "address"),
                    kind: opt_s(x, "type").unwrap_or_else(|| "required".into()),
                    response: x["status"]["response"].as_str().unwrap_or("none").to_string(),
                })
                .collect()
        })
        .unwrap_or_default();
    let online_url = v["onlineMeeting"]["joinUrl"]
        .as_str()
        .filter(|x| !x.is_empty())
        .map(str::to_string)
        .or_else(|| opt_s(v, "onlineMeetingUrl"));
    Event {
        id: s(v, "id"),
        account_id: account_id.to_string(),
        calendar_id: calendar_id.to_string(),
        subject: s(v, "subject"),
        start: dt_tz(&v["start"]),
        end: dt_tz(&v["end"]),
        is_all_day: v["isAllDay"].as_bool().unwrap_or(false),
        location: s(&v["location"], "displayName"),
        organizer: v.get("organizer").and_then(address),
        attendees,
        show_as: opt_s(v, "showAs").unwrap_or_else(|| "busy".into()),
        categories: string_list(v, "categories"),
        is_organizer: v["isOrganizer"].as_bool().unwrap_or(true),
        response_status: v["responseStatus"]["response"]
            .as_str()
            .unwrap_or("none")
            .to_string(),
        body_preview: s(v, "bodyPreview"),
        is_online_meeting: v["isOnlineMeeting"].as_bool().unwrap_or(false),
        online_meeting_url: online_url,
        recurrence: recurrence_from_graph(&v["recurrence"]),
        series_master_id: opt_s(v, "seriesMasterId"),
        kind: opt_s(v, "type").unwrap_or_else(|| "singleInstance".into()),
        is_reminder_on: v["isReminderOn"].as_bool().unwrap_or(false),
        reminder_minutes: v["reminderMinutesBeforeStart"].as_i64().unwrap_or(15),
        importance: opt_s(v, "importance").unwrap_or_else(|| "normal".into()),
        sensitivity: opt_s(v, "sensitivity").unwrap_or_else(|| "normal".into()),
        is_cancelled: v["isCancelled"].as_bool().unwrap_or(false),
        web_link: opt_s(v, "webLink"),
    }
}

/// EventInput → Graph event JSON (create/update).
pub fn event_input_to_graph(input: &EventInput) -> Value {
    let (start, end) = if input.is_all_day {
        // All-day events need midnight boundaries.
        (date_midnight(&input.start), date_midnight(&input.end))
    } else {
        (input.start.clone(), input.end.clone())
    };
    let attendees: Vec<Value> = input
        .attendees
        .iter()
        .filter(|a| !a.address.trim().is_empty())
        .map(|a| {
            json!({
                "emailAddress": {"name": if a.name.is_empty() { &a.address } else { &a.name }, "address": a.address.trim()},
                "type": a.kind,
            })
        })
        .collect();
    let mut v = json!({
        "subject": input.subject,
        "start": {"dateTime": start, "timeZone": input.time_zone},
        "end": {"dateTime": end, "timeZone": input.time_zone},
        "isAllDay": input.is_all_day,
        "location": {"displayName": input.location},
        "body": {"contentType": "html", "content": input.body_html},
        "attendees": attendees,
        "showAs": input.show_as,
        "categories": input.categories,
        "isReminderOn": input.is_reminder_on,
        "reminderMinutesBeforeStart": input.reminder_minutes,
        "isOnlineMeeting": input.is_online_meeting,
        "sensitivity": input.sensitivity,
        "importance": input.importance,
    });
    if input.is_online_meeting {
        v["onlineMeetingProvider"] = json!("teamsForBusiness");
    }
    v["recurrence"] = match &input.recurrence {
        Some(r) => recurrence_to_graph(r, &input.time_zone),
        None => Value::Null,
    };
    v
}

fn date_midnight(ts: &str) -> String {
    let date = ts.get(..10).unwrap_or(ts);
    format!("{date}T00:00:00")
}

pub fn auto_reply_from_graph(v: &Value) -> AutoReply {
    let start = &v["scheduledStartDateTime"];
    let end = &v["scheduledEndDateTime"];
    AutoReply {
        status: opt_s(v, "status").unwrap_or_else(|| "disabled".into()),
        internal_reply_message: s(v, "internalReplyMessage"),
        external_reply_message: s(v, "externalReplyMessage"),
        scheduled_start: (!start.is_null() && !s(start, "dateTime").is_empty()).then(|| dt_tz(start)),
        scheduled_end: (!end.is_null() && !s(end, "dateTime").is_empty()).then(|| dt_tz(end)),
        external_audience: opt_s(v, "externalAudience").unwrap_or_else(|| "all".into()),
    }
}

pub fn auto_reply_to_graph(a: &AutoReply) -> Value {
    let mut v = json!({
        "status": a.status,
        "internalReplyMessage": a.internal_reply_message,
        "externalReplyMessage": a.external_reply_message,
        "externalAudience": a.external_audience,
    });
    let to_dt = |ts: &str| -> Value {
        let dt = DateTime::parse_from_rfc3339(ts)
            .map(|d| d.with_timezone(&Utc).format("%Y-%m-%dT%H:%M:%S").to_string())
            .unwrap_or_else(|_| ts.to_string());
        json!({"dateTime": dt, "timeZone": "UTC"})
    };
    if let Some(sx) = &a.scheduled_start {
        v["scheduledStartDateTime"] = to_dt(sx);
    }
    if let Some(e) = &a.scheduled_end {
        v["scheduledEndDateTime"] = to_dt(e);
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_timestamps() {
        assert_eq!(normalize_utc("2026-10-15T14:40:00Z"), "2026-10-15T14:40:00Z");
        assert_eq!(normalize_utc("2026-10-15T14:40:00.0000000"), "2026-10-15T14:40:00Z");
        assert_eq!(normalize_utc("2026-10-15T16:40:00+02:00"), "2026-10-15T14:40:00Z");
    }

    #[test]
    fn maps_message_summary() {
        let v = json!({
            "id": "m1", "conversationId": "c1", "subject": "Hallo",
            "from": {"emailAddress": {"name": "A", "address": "a@x"}},
            "toRecipients": [{"emailAddress": {"name": "B", "address": "b@x"}}],
            "receivedDateTime": "2026-10-15T14:40:00Z", "isRead": false,
            "importance": "high",
            "hasAttachments": true, "categories": ["Info"], "bodyPreview": "…",
            "flag": {"flagStatus": "flagged", "dueDateTime": {"dateTime": "2026-10-21T06:00:00.0000000", "timeZone": "UTC"}},
            "parentFolderId": "f1"
        });
        let m = message_summary(&v, "mb", "other");
        assert_eq!(m.folder_id, "f1");
        assert_eq!(m.from.unwrap().address, "a@x");
        assert!(!m.is_read);
        assert_eq!(m.flag, "flagged");
        assert_eq!(m.categories, vec!["Info"]);
        assert_eq!(m.due_at.as_deref(), Some("2026-10-21T06:00:00Z"));
    }

    #[test]
    fn text_body_is_escaped() {
        let v = json!({"body": {"contentType": "text", "content": "<b>x</b>"}});
        assert!(body_html(&v).contains("&lt;b&gt;x&lt;/b&gt;"));
    }

    #[test]
    fn recurrence_roundtrip() {
        let r = Recurrence {
            pattern: "weekly".into(),
            interval: 1,
            days_of_week: vec!["monday".into()],
            day_of_month: 0,
            month: 0,
            start_date: "2026-10-19".into(),
            end_date: Some("2026-12-31".into()),
            occurrences: None,
        };
        let g = recurrence_to_graph(&r, "Europe/Berlin");
        assert_eq!(g["range"]["type"], "endDate");
        let back = recurrence_from_graph(&g).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn all_day_input_uses_midnight() {
        let input: EventInput = serde_json::from_value(json!({
            "subject": "Urlaub", "start": "2026-10-20T10:00:00", "end": "2026-10-21T10:00:00",
            "timeZone": "Europe/Berlin", "isAllDay": true
        }))
        .unwrap();
        let g = event_input_to_graph(&input);
        assert_eq!(g["start"]["dateTime"], "2026-10-20T00:00:00");
        assert_eq!(g["end"]["dateTime"], "2026-10-21T00:00:00");
        assert!(g["recurrence"].is_null());
    }
}
