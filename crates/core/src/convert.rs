//! Mapping between Microsoft Graph JSON and the UI model.

use chrono::{DateTime, NaiveDateTime, SecondsFormat, TimeZone, Utc};
use serde_json::{json, Value};

use crate::model::*;

/// `$select` used for message lists and delta queries.
pub const MESSAGE_SUMMARY_SELECT: &str = "id,conversationId,subject,from,toRecipients,receivedDateTime,isRead,isDraft,importance,flag,hasAttachments,categories,bodyPreview,parentFolderId";

pub const MESSAGE_DETAIL_SELECT: &str = "id,conversationId,subject,from,toRecipients,ccRecipients,bccRecipients,replyTo,receivedDateTime,sentDateTime,isRead,isDraft,importance,flag,hasAttachments,categories,bodyPreview,parentFolderId,body,internetMessageId,webLink,isReadReceiptRequested";

pub const EVENT_SELECT: &str = "id,subject,start,end,isAllDay,location,organizer,attendees,showAs,categories,isOrganizer,responseStatus,bodyPreview,isOnlineMeeting,onlineMeeting,recurrence,seriesMasterId,type,isReminderOn,reminderMinutesBeforeStart,importance,sensitivity,isCancelled,webLink";

pub fn s(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

fn opt_s(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .filter(|x| !x.is_empty())
        .map(str::to_string)
}

pub fn address(v: &Value) -> Option<Address> {
    let ea = v.get("emailAddress").filter(|e| e.is_object())?;
    let a = Address {
        name: s(ea, "name").trim().to_string(),
        address: s(ea, "address").trim().to_string(),
    };
    (!a.name.is_empty() || !a.address.is_empty()).then_some(a)
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

/// Windows time zone ids (as used by Exchange/Graph) → IANA names, after the
/// CLDR `windowsZones` table (territory "001").
const WINDOWS_ZONES: &[(&str, &str)] = &[
    ("Dateline Standard Time", "Etc/GMT+12"),
    ("UTC-11", "Etc/GMT+11"),
    ("Aleutian Standard Time", "America/Adak"),
    ("Hawaiian Standard Time", "Pacific/Honolulu"),
    ("Marquesas Standard Time", "Pacific/Marquesas"),
    ("Alaskan Standard Time", "America/Anchorage"),
    ("UTC-09", "Etc/GMT+9"),
    ("Pacific Standard Time (Mexico)", "America/Tijuana"),
    ("UTC-08", "Etc/GMT+8"),
    ("Pacific Standard Time", "America/Los_Angeles"),
    ("US Mountain Standard Time", "America/Phoenix"),
    ("Mountain Standard Time (Mexico)", "America/Mazatlan"),
    ("Mountain Standard Time", "America/Denver"),
    ("Yukon Standard Time", "America/Whitehorse"),
    ("Central America Standard Time", "America/Guatemala"),
    ("Central Standard Time", "America/Chicago"),
    ("Easter Island Standard Time", "Pacific/Easter"),
    ("Central Standard Time (Mexico)", "America/Mexico_City"),
    ("Canada Central Standard Time", "America/Regina"),
    ("SA Pacific Standard Time", "America/Bogota"),
    ("Eastern Standard Time (Mexico)", "America/Cancun"),
    ("Eastern Standard Time", "America/New_York"),
    ("Haiti Standard Time", "America/Port-au-Prince"),
    ("Cuba Standard Time", "America/Havana"),
    ("US Eastern Standard Time", "America/Indiana/Indianapolis"),
    ("Turks And Caicos Standard Time", "America/Grand_Turk"),
    ("Paraguay Standard Time", "America/Asuncion"),
    ("Atlantic Standard Time", "America/Halifax"),
    ("Venezuela Standard Time", "America/Caracas"),
    ("Central Brazilian Standard Time", "America/Cuiaba"),
    ("SA Western Standard Time", "America/La_Paz"),
    ("Pacific SA Standard Time", "America/Santiago"),
    ("Newfoundland Standard Time", "America/St_Johns"),
    ("Tocantins Standard Time", "America/Araguaina"),
    ("E. South America Standard Time", "America/Sao_Paulo"),
    ("SA Eastern Standard Time", "America/Cayenne"),
    ("Argentina Standard Time", "America/Argentina/Buenos_Aires"),
    ("Greenland Standard Time", "America/Nuuk"),
    ("Montevideo Standard Time", "America/Montevideo"),
    ("Magallanes Standard Time", "America/Punta_Arenas"),
    ("Saint Pierre Standard Time", "America/Miquelon"),
    ("Bahia Standard Time", "America/Bahia"),
    ("UTC-02", "Etc/GMT+2"),
    ("Mid-Atlantic Standard Time", "Etc/GMT+2"),
    ("Azores Standard Time", "Atlantic/Azores"),
    ("Cape Verde Standard Time", "Atlantic/Cape_Verde"),
    ("UTC", "Etc/UTC"),
    ("Coordinated Universal Time", "Etc/UTC"),
    ("GMT Standard Time", "Europe/London"),
    ("Greenwich Standard Time", "Atlantic/Reykjavik"),
    ("Sao Tome Standard Time", "Africa/Sao_Tome"),
    ("Morocco Standard Time", "Africa/Casablanca"),
    ("W. Europe Standard Time", "Europe/Berlin"),
    ("Central Europe Standard Time", "Europe/Budapest"),
    ("Romance Standard Time", "Europe/Paris"),
    ("Central European Standard Time", "Europe/Warsaw"),
    ("W. Central Africa Standard Time", "Africa/Lagos"),
    ("Jordan Standard Time", "Asia/Amman"),
    ("GTB Standard Time", "Europe/Bucharest"),
    ("Middle East Standard Time", "Asia/Beirut"),
    ("Egypt Standard Time", "Africa/Cairo"),
    ("E. Europe Standard Time", "Europe/Chisinau"),
    ("Syria Standard Time", "Asia/Damascus"),
    ("West Bank Standard Time", "Asia/Hebron"),
    ("South Africa Standard Time", "Africa/Johannesburg"),
    ("FLE Standard Time", "Europe/Kiev"),
    ("Israel Standard Time", "Asia/Jerusalem"),
    ("South Sudan Standard Time", "Africa/Juba"),
    ("Kaliningrad Standard Time", "Europe/Kaliningrad"),
    ("Sudan Standard Time", "Africa/Khartoum"),
    ("Libya Standard Time", "Africa/Tripoli"),
    ("Namibia Standard Time", "Africa/Windhoek"),
    ("Arabic Standard Time", "Asia/Baghdad"),
    ("Turkey Standard Time", "Europe/Istanbul"),
    ("Arab Standard Time", "Asia/Riyadh"),
    ("Belarus Standard Time", "Europe/Minsk"),
    ("Russian Standard Time", "Europe/Moscow"),
    ("E. Africa Standard Time", "Africa/Nairobi"),
    ("Volgograd Standard Time", "Europe/Volgograd"),
    ("Iran Standard Time", "Asia/Tehran"),
    ("Arabian Standard Time", "Asia/Dubai"),
    ("Astrakhan Standard Time", "Europe/Astrakhan"),
    ("Azerbaijan Standard Time", "Asia/Baku"),
    ("Russia Time Zone 3", "Europe/Samara"),
    ("Mauritius Standard Time", "Indian/Mauritius"),
    ("Saratov Standard Time", "Europe/Saratov"),
    ("Georgian Standard Time", "Asia/Tbilisi"),
    ("Caucasus Standard Time", "Asia/Yerevan"),
    ("Afghanistan Standard Time", "Asia/Kabul"),
    ("West Asia Standard Time", "Asia/Tashkent"),
    ("Ekaterinburg Standard Time", "Asia/Yekaterinburg"),
    ("Pakistan Standard Time", "Asia/Karachi"),
    ("Qyzylorda Standard Time", "Asia/Qyzylorda"),
    ("India Standard Time", "Asia/Kolkata"),
    ("Sri Lanka Standard Time", "Asia/Colombo"),
    ("Nepal Standard Time", "Asia/Kathmandu"),
    ("Central Asia Standard Time", "Asia/Almaty"),
    ("Bangladesh Standard Time", "Asia/Dhaka"),
    ("Omsk Standard Time", "Asia/Omsk"),
    ("Myanmar Standard Time", "Asia/Yangon"),
    ("SE Asia Standard Time", "Asia/Bangkok"),
    ("Altai Standard Time", "Asia/Barnaul"),
    ("W. Mongolia Standard Time", "Asia/Hovd"),
    ("North Asia Standard Time", "Asia/Krasnoyarsk"),
    ("N. Central Asia Standard Time", "Asia/Novosibirsk"),
    ("Tomsk Standard Time", "Asia/Tomsk"),
    ("China Standard Time", "Asia/Shanghai"),
    ("North Asia East Standard Time", "Asia/Irkutsk"),
    ("Singapore Standard Time", "Asia/Singapore"),
    ("W. Australia Standard Time", "Australia/Perth"),
    ("Taipei Standard Time", "Asia/Taipei"),
    ("Ulaanbaatar Standard Time", "Asia/Ulaanbaatar"),
    ("Aus Central W. Standard Time", "Australia/Eucla"),
    ("Transbaikal Standard Time", "Asia/Chita"),
    ("Tokyo Standard Time", "Asia/Tokyo"),
    ("North Korea Standard Time", "Asia/Pyongyang"),
    ("Korea Standard Time", "Asia/Seoul"),
    ("Yakutsk Standard Time", "Asia/Yakutsk"),
    ("Cen. Australia Standard Time", "Australia/Adelaide"),
    ("AUS Central Standard Time", "Australia/Darwin"),
    ("E. Australia Standard Time", "Australia/Brisbane"),
    ("AUS Eastern Standard Time", "Australia/Sydney"),
    ("West Pacific Standard Time", "Pacific/Port_Moresby"),
    ("Tasmania Standard Time", "Australia/Hobart"),
    ("Vladivostok Standard Time", "Asia/Vladivostok"),
    ("Lord Howe Standard Time", "Australia/Lord_Howe"),
    ("Bougainville Standard Time", "Pacific/Bougainville"),
    ("Russia Time Zone 10", "Asia/Srednekolymsk"),
    ("Magadan Standard Time", "Asia/Magadan"),
    ("Norfolk Standard Time", "Pacific/Norfolk"),
    ("Sakhalin Standard Time", "Asia/Sakhalin"),
    ("Central Pacific Standard Time", "Pacific/Guadalcanal"),
    ("Russia Time Zone 11", "Asia/Kamchatka"),
    ("New Zealand Standard Time", "Pacific/Auckland"),
    ("UTC+12", "Etc/GMT-12"),
    ("Fiji Standard Time", "Pacific/Fiji"),
    ("Chatham Islands Standard Time", "Pacific/Chatham"),
    ("UTC+13", "Etc/GMT-13"),
    ("Tonga Standard Time", "Pacific/Tongatapu"),
    ("Samoa Standard Time", "Pacific/Apia"),
    ("Line Islands Standard Time", "Pacific/Kiritimati"),
];

/// Resolves a Graph `timeZone` value: IANA name, Windows id, or a UTC alias.
/// `None` for unknown/custom zones (e.g. "Customized Time Zone").
pub fn resolve_tz(name: &str) -> Option<chrono_tz::Tz> {
    let n = name.trim();
    if n.is_empty()
        || n.eq_ignore_ascii_case("UTC")
        || n.eq_ignore_ascii_case("GMT")
        || n.eq_ignore_ascii_case("Etc/GMT")
        || n.eq_ignore_ascii_case("Z")
        || n.eq_ignore_ascii_case("tzone://Microsoft/Utc")
    {
        return Some(chrono_tz::UTC);
    }
    if let Some((_, iana)) = WINDOWS_ZONES.iter().find(|(w, _)| w.eq_ignore_ascii_case(n)) {
        return iana.parse().ok();
    }
    n.parse::<chrono_tz::Tz>().ok()
}

/// Wall time in `tz` → UTC. Times in a DST gap move to the first valid wall
/// time after it, ambiguous times take the first (summer time) instant.
pub fn local_to_utc(local: NaiveDateTime, tz: chrono_tz::Tz) -> DateTime<Utc> {
    use chrono::LocalResult;
    match tz.from_local_datetime(&local) {
        LocalResult::Single(d) => d.with_timezone(&Utc),
        LocalResult::Ambiguous(a, _) => a.with_timezone(&Utc),
        LocalResult::None => {
            // Move to the first wall time after the gap.
            (1..=180)
                .find_map(|m| tz.from_local_datetime(&(local + chrono::Duration::minutes(m))).earliest())
                .map(|d| d.with_timezone(&Utc))
                .unwrap_or_else(|| Utc.from_utc_datetime(&local))
        }
    }
}

fn parse_naive(ts: &str) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(ts, "%Y-%m-%dT%H:%M:%S%.f")
        .or_else(|_| NaiveDateTime::parse_from_str(ts, "%Y-%m-%dT%H:%M"))
        .ok()
}

/// Whether a Graph `dateTimeTimeZone` can be converted without a round trip.
pub fn dt_tz_resolvable(v: &Value) -> bool {
    let dt = s(v, "dateTime");
    DateTime::parse_from_rfc3339(&dt).is_ok() || resolve_tz(&s(v, "timeZone")).is_some()
}

/// Graph `dateTimeTimeZone` → RFC3339 UTC string.
///
/// We request `Prefer: outlook.timezone="UTC"` where Graph honours it, but
/// POST/PATCH responses, flags and mailbox settings come back in the zone the
/// value was written in (IANA or Windows id), so the zone is applied here.
/// Unknown zones fall back to treating the wall time as UTC.
pub fn dt_tz(v: &Value) -> String {
    let dt = s(v, "dateTime");
    if dt.is_empty() || DateTime::parse_from_rfc3339(&dt).is_ok() {
        return normalize_utc(&dt);
    }
    let tz_name = s(v, "timeZone");
    match (parse_naive(&dt), resolve_tz(&tz_name)) {
        (Some(local), Some(tz)) => local_to_utc(local, tz).to_rfc3339_opts(SecondsFormat::Secs, true),
        (_, None) => {
            tracing::warn!("Unbekannte Zeitzone \"{tz_name}\" – Zeit wird als UTC gelesen");
            normalize_utc(&dt)
        }
        _ => normalize_utc(&dt),
    }
}

/// All-day events are floating: midnight to midnight in whatever zone Graph
/// labels them with. The date must not shift, so the wall time is kept.
fn dt_floating(v: &Value) -> String {
    let dt = s(v, "dateTime");
    match parse_naive(&dt) {
        Some(n) => Utc.from_utc_datetime(&n).to_rfc3339_opts(SecondsFormat::Secs, true),
        None => normalize_utc(&dt),
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
        // A completed flag keeps its old due date on the server; it is no longer due.
        due_at: v["flag"]
            .get("dueDateTime")
            .filter(|d| !s(d, "dateTime").is_empty())
            .filter(|_| v["flag"]["flagStatus"].as_str() != Some("complete"))
            .map(dt_tz),
        recurrence: recurrence_of(&string_list(v, "categories")),
    }
}

/// True for a delta item that carries only some properties (Graph sends just the
/// changed ones at times, e.g. after a read-state change in another client).
pub fn is_partial_message(v: &Value) -> bool {
    v.get("receivedDateTime").is_none()
}

/// Applies the properties present in a partial Graph item to a cached summary.
pub fn merge_message_summary(old: &MessageSummary, v: &Value) -> MessageSummary {
    let mut m = old.clone();
    let has = |k: &str| v.get(k).is_some_and(|x| !x.is_null());
    if has("subject") { m.subject = s(v, "subject"); }
    if has("from") { m.from = v.get("from").and_then(address); }
    if has("toRecipients") { m.to = addresses(v, "toRecipients"); }
    if has("conversationId") { m.conversation_id = s(v, "conversationId"); }
    if has("isRead") { m.is_read = v["isRead"].as_bool().unwrap_or(m.is_read); }
    if has("isDraft") { m.is_draft = v["isDraft"].as_bool().unwrap_or(m.is_draft); }
    if has("importance") { m.importance = s(v, "importance"); }
    if has("hasAttachments") { m.has_attachments = v["hasAttachments"].as_bool().unwrap_or(m.has_attachments); }
    if has("bodyPreview") { m.preview = s(v, "bodyPreview"); }
    if has("categories") {
        m.categories = string_list(v, "categories");
        m.recurrence = recurrence_of(&m.categories);
    }
    if has("flag") {
        let full = message_summary(v, &m.mailbox_id, &m.folder_id);
        m.flag = full.flag;
        m.due_at = full.due_at;
    }
    m
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
        group: None,
        group_id: None,
        owner: Some(Address {
            name: s(&v["owner"], "name").trim().to_string(),
            address: s(&v["owner"], "address").trim().to_string(),
        })
        .filter(|a| !a.address.is_empty()),
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
    let p = v.get("pattern")?;
    let r = &v["range"];
    let pattern = s(p, "type");
    if pattern.is_empty() {
        return None;
    }
    let range_type = s(r, "type");
    let relative = pattern.starts_with("relative");
    Some(Recurrence {
        interval: p["interval"].as_i64().unwrap_or(1).max(1),
        days_of_week: string_list(p, "daysOfWeek"),
        day_of_month: p["dayOfMonth"].as_i64().unwrap_or(0),
        month: p["month"].as_i64().unwrap_or(0),
        index: relative.then(|| opt_s(p, "index").unwrap_or_else(|| "first".into())),
        pattern,
        start_date: s(r, "startDate"),
        end_date: if range_type == "endDate" {
            opt_s(r, "endDate").filter(|d| d != "0001-01-01")
        } else {
            None
        },
        occurrences: if range_type == "numbered" {
            r["numberOfOccurrences"].as_i64().filter(|n| *n > 0)
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
    let index = || r.index.clone().filter(|i| !i.is_empty()).unwrap_or_else(|| "first".into());
    match r.pattern.as_str() {
        "weekly" => {
            pattern["daysOfWeek"] = json!(r.days_of_week);
            pattern["firstDayOfWeek"] = json!("monday");
        }
        "absoluteMonthly" => pattern["dayOfMonth"] = json!(r.day_of_month.clamp(1, 31)),
        "relativeMonthly" => {
            pattern["daysOfWeek"] = json!(r.days_of_week);
            pattern["index"] = json!(index());
        }
        "absoluteYearly" => {
            pattern["dayOfMonth"] = json!(r.day_of_month.clamp(1, 31));
            pattern["month"] = json!(r.month.clamp(1, 12));
        }
        "relativeYearly" => {
            pattern["daysOfWeek"] = json!(r.days_of_week);
            pattern["index"] = json!(index());
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

/// Whether two recurrences describe the same series (ignoring fields that do
/// not apply to the pattern). Re-sending an unchanged recurrence would make
/// Exchange drop all exceptions of the series.
pub fn same_recurrence(a: &Recurrence, b: &Recurrence) -> bool {
    let norm = |r: &Recurrence| {
        let mut v = recurrence_to_graph(r, "");
        if let Some(days) = v["pattern"]["daysOfWeek"].as_array_mut() {
            let mut d: Vec<String> = days.iter().filter_map(|x| x.as_str().map(str::to_lowercase)).collect();
            d.sort();
            d.dedup();
            *days = d.into_iter().map(Value::from).collect();
        }
        v
    };
    norm(a) == norm(b)
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
    let is_all_day = v["isAllDay"].as_bool().unwrap_or(false);
    let when = |key: &str| if is_all_day { dt_floating(&v[key]) } else { dt_tz(&v[key]) };
    Event {
        id: s(v, "id"),
        account_id: account_id.to_string(),
        calendar_id: calendar_id.to_string(),
        subject: s(v, "subject"),
        start: when("start"),
        end: when("end"),
        is_all_day,
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
            index: None,
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

#[cfg(test)]
mod edge_tests {
    use super::*;

    fn dtz(dt: &str, tz: &str) -> String {
        dt_tz(&json!({"dateTime": dt, "timeZone": tz}))
    }

    #[test]
    fn every_windows_zone_resolves() {
        for (win, iana) in WINDOWS_ZONES {
            assert!(iana.parse::<chrono_tz::Tz>().is_ok(), "{win} → {iana}");
            assert!(resolve_tz(win).is_some(), "{win}");
        }
        assert_eq!(resolve_tz("w. europe standard time"), Some(chrono_tz::Europe::Berlin));
        assert_eq!(resolve_tz(" Europe/Berlin "), Some(chrono_tz::Europe::Berlin));
        for utc in ["", "UTC", "utc", "Etc/GMT", "GMT", "tzone://Microsoft/Utc", "Coordinated Universal Time"] {
            assert!(resolve_tz(utc).is_some_and(|z| z.name().contains("UTC")), "{utc:?}");
        }
        assert!(resolve_tz("Customized Time Zone").is_none());
        assert!(resolve_tz("Mars/Olympus_Mons").is_none());
    }

    #[test]
    fn date_time_time_zone_conversion() {
        // The live bug: wall time in the event's zone must not be read as UTC.
        assert_eq!(dtz("2026-11-19T10:00:00.0000000", "Europe/Berlin"), "2026-11-19T09:00:00Z");
        assert_eq!(dtz("2026-11-19T10:00:00.0000000", "W. Europe Standard Time"), "2026-11-19T09:00:00Z");
        assert_eq!(dtz("2026-07-01T10:00:00", "W. Europe Standard Time"), "2026-07-01T08:00:00Z");
        assert_eq!(dtz("2026-11-19T10:00:00", "Eastern Standard Time"), "2026-11-19T15:00:00Z");
        assert_eq!(dtz("2026-11-19T10:00:00", "India Standard Time"), "2026-11-19T04:30:00Z");
        assert_eq!(dtz("2026-11-19T10:00:00", "UTC"), "2026-11-19T10:00:00Z");
        assert_eq!(dtz("2026-11-19T10:00", "UTC"), "2026-11-19T10:00:00Z");
        // Explicit offsets win over the label.
        assert_eq!(dtz("2026-11-19T10:00:00+01:00", "Pacific Standard Time"), "2026-11-19T09:00:00Z");
        // Unknown zone: wall time is kept (better than failing).
        assert_eq!(dtz("2026-11-19T10:00:00", "Customized Time Zone"), "2026-11-19T10:00:00Z");
        assert_eq!(dtz("", "Europe/Berlin"), "");
        assert_eq!(dt_tz(&Value::Null), "");
        assert_eq!(dtz("kaputt", "Europe/Berlin"), "kaputt");
        assert!(dt_tz_resolvable(&json!({"dateTime": "2026-11-19T10:00:00", "timeZone": "Romance Standard Time"})));
        assert!(!dt_tz_resolvable(&json!({"dateTime": "2026-11-19T10:00:00", "timeZone": "Customized Time Zone"})));
    }

    #[test]
    fn dst_gaps_and_overlaps() {
        let berlin = chrono_tz::Europe::Berlin;
        let n = |s: &str| NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S").unwrap();
        // 02:30 does not exist on 2027-03-28: moves to 03:00 CEST.
        assert_eq!(local_to_utc(n("2027-03-28T02:30:00"), berlin).to_rfc3339(), "2027-03-28T01:00:00+00:00");
        // 02:30 exists twice on 2026-10-25: the first (CEST) instant.
        assert_eq!(local_to_utc(n("2026-10-25T02:30:00"), berlin).to_rfc3339(), "2026-10-25T00:30:00+00:00");
        assert_eq!(dtz("2026-10-25T03:00:00", "Europe/Berlin"), "2026-10-25T02:00:00Z");
        assert_eq!(dtz("2027-03-28T03:00:00", "Europe/Berlin"), "2027-03-28T01:00:00Z");
    }

    #[test]
    fn all_day_events_are_floating() {
        let v = json!({"id": "e", "isAllDay": true,
            "start": {"dateTime": "2026-10-24T00:00:00.0000000", "timeZone": "W. Europe Standard Time"},
            "end": {"dateTime": "2026-10-27T00:00:00.0000000", "timeZone": "W. Europe Standard Time"}});
        let e = event(&v, "a", "c");
        assert_eq!((e.start.as_str(), e.end.as_str()), ("2026-10-24T00:00:00Z", "2026-10-27T00:00:00Z"));
        let mut timed = v.clone();
        timed["isAllDay"] = json!(false);
        assert_eq!(event(&timed, "a", "c").start, "2026-10-23T22:00:00Z");
    }

    #[test]
    fn missing_and_null_fields() {
        let m = message_summary(&json!({}), "mb", "inbox");
        assert_eq!(m.folder_id, "inbox");
        assert!(m.from.is_none() && m.to.is_empty() && m.categories.is_empty());
        assert_eq!((m.importance.as_str(), m.flag.as_str()), ("normal", "notFlagged"));
        assert!(m.is_read && !m.is_draft && m.due_at.is_none() && m.recurrence.is_none());
        let m = message_summary(
            &json!({"from": null, "toRecipients": null, "categories": null, "flag": null, "importance": null,
                    "subject": null, "receivedDateTime": null, "isRead": null, "parentFolderId": ""}),
            "mb",
            "x",
        );
        assert_eq!(m.folder_id, "x");
        assert!(m.from.is_none() && m.subject.is_empty() && m.received_at.is_empty());

        let e = event(&json!({"id": "e1", "start": null, "location": null, "organizer": null, "attendees": null, "recurrence": null}), "a", "c");
        assert!(e.start.is_empty() && e.location.is_empty() && e.organizer.is_none() && e.attendees.is_empty());
        assert!(e.recurrence.is_none());
        assert_eq!((e.kind.as_str(), e.show_as.as_str(), e.response_status.as_str()), ("singleInstance", "busy", "none"));
        // Attendee without status/type.
        let e = event(&json!({"attendees": [{"emailAddress": {"address": "x@y"}}]}), "a", "c");
        assert_eq!((e.attendees[0].kind.as_str(), e.attendees[0].response.as_str()), ("required", "none"));
        // onlineMeeting null but legacy url present.
        let e = event(&json!({"onlineMeeting": null, "onlineMeetingUrl": "https://teams/x"}), "a", "c");
        assert_eq!(e.online_meeting_url.as_deref(), Some("https://teams/x"));

        let a = attachment(&json!({"size": null}));
        assert_eq!((a.content_type.as_str(), a.size), ("application/octet-stream", 0));
        let c = category(&json!({"displayName": "Info"}));
        assert_eq!(c.color, "none");
        let cal = calendar(&json!({"name": "K", "hexColor": "", "color": "lightBlue"}), "a");
        assert_eq!(cal.color.as_deref(), Some("#a6d1f5"));
        let cal = calendar(&json!({"name": "K", "color": "auto"}), "a");
        assert!(cal.color.is_none() && cal.can_edit && !cal.is_default);
        let ar = auto_reply_from_graph(&json!({}));
        assert_eq!((ar.status.as_str(), ar.external_audience.as_str()), ("disabled", "all"));
        assert!(ar.scheduled_start.is_none());
    }

    #[test]
    fn odd_addresses() {
        let v = json!({
            "from": {"emailAddress": null},
            "toRecipients": [
                {"emailAddress": {"name": " Erika ", "address": " erika@x.example "}},
                {"emailAddress": {"name": "", "address": ""}},
                {"emailAddress": null},
                {"foo": 1},
                {"emailAddress": {"name": "Nur Name"}},
                {"emailAddress": {"address": "/O=EXCHANGELABS/OU=EXCHANGE ADMINISTRATIVE GROUP/CN=RECIPIENTS/CN=MAX"}}
            ]
        });
        let m = message_summary(&v, "mb", "f");
        assert!(m.from.is_none());
        let to: Vec<(String, String)> = m.to.into_iter().map(|a| (a.name, a.address)).collect();
        assert_eq!(to.len(), 3);
        assert_eq!(to[0], ("Erika".into(), "erika@x.example".into()));
        assert_eq!(to[1].0, "Nur Name");
        // Outgoing: blank addresses dropped, name falls back to the address.
        let out = recipients_json(&[
            Address { name: "".into(), address: " a@b.example ".into() },
            Address { name: "X".into(), address: "  ".into() },
            Address { name: " Ünïcødé 😀 ".into(), address: "u@b.example".into() },
        ]);
        assert_eq!(out.as_array().unwrap().len(), 2);
        assert_eq!(out[0]["emailAddress"]["name"], "a@b.example");
        assert_eq!(out[1]["emailAddress"]["name"], "Ünïcødé 😀");
    }

    #[test]
    fn flags_importance_and_categories() {
        let base = |flag: Value| json!({"flag": flag, "importance": "high", "categories": ["Info", 3, null, "[#weekly]", "[K_ToDo]"]});
        let m = message_summary(
            &base(json!({"flagStatus": "flagged", "dueDateTime": {"dateTime": "2026-10-21T08:00:00.0000000", "timeZone": "W. Europe Standard Time"}})),
            "mb",
            "f",
        );
        assert_eq!(m.due_at.as_deref(), Some("2026-10-21T06:00:00Z"));
        assert_eq!(m.importance, "high");
        assert_eq!(m.categories, ["Info", "[#weekly]", "[K_ToDo]"]);
        assert_eq!(m.recurrence.as_deref(), Some("weekly"));
        // Completed flags keep a due date on the server, but nothing is due any more.
        let m = message_summary(
            &base(json!({"flagStatus": "complete", "dueDateTime": {"dateTime": "2026-10-21T08:00:00", "timeZone": "UTC"}})),
            "mb",
            "f",
        );
        assert_eq!(m.flag, "complete");
        assert!(m.due_at.is_none());
        let m = message_summary(&base(json!({"flagStatus": "flagged", "dueDateTime": {"dateTime": "", "timeZone": "UTC"}})), "mb", "f");
        assert!(m.due_at.is_none());
        // Unknown recurrence tags are plain categories.
        let m = message_summary(&json!({"categories": ["[#hourly]"]}), "mb", "f");
        assert!(m.recurrence.is_none());
    }

    fn rec(pattern: &str) -> Recurrence {
        Recurrence {
            pattern: pattern.into(),
            interval: 2,
            days_of_week: vec![],
            day_of_month: 0,
            month: 0,
            index: None,
            start_date: "2026-11-01".into(),
            end_date: None,
            occurrences: None,
        }
    }

    #[test]
    fn recurrence_patterns_roundtrip() {
        let mut cases = vec![rec("daily")];
        let mut w = rec("weekly");
        w.days_of_week = vec!["monday".into(), "friday".into()];
        w.occurrences = Some(10);
        cases.push(w);
        let mut am = rec("absoluteMonthly");
        am.day_of_month = 31;
        am.end_date = Some("2027-12-31".into());
        cases.push(am);
        let mut rm = rec("relativeMonthly");
        rm.days_of_week = vec!["tuesday".into()];
        rm.index = Some("last".into());
        cases.push(rm);
        let mut ay = rec("absoluteYearly");
        ay.day_of_month = 29;
        ay.month = 2;
        cases.push(ay);
        let mut ry = rec("relativeYearly");
        ry.days_of_week = vec!["friday".into()];
        ry.index = Some("first".into());
        ry.month = 3;
        cases.push(ry);
        for r in cases {
            let g = recurrence_to_graph(&r, "Europe/Berlin");
            assert_eq!(g["range"]["recurrenceTimeZone"], "Europe/Berlin");
            let back = recurrence_from_graph(&g).unwrap();
            assert_eq!(back, r, "{}", r.pattern);
            assert!(same_recurrence(&back, &r));
        }
        // Relative patterns without index default to "first" on the wire.
        let mut rm = rec("relativeMonthly");
        rm.days_of_week = vec!["monday".into()];
        assert_eq!(recurrence_to_graph(&rm, "UTC")["pattern"]["index"], "first");
        // Out-of-range values are clamped instead of producing invalid requests.
        let mut am = rec("absoluteYearly");
        am.day_of_month = 40;
        am.month = 13;
        am.interval = 0;
        let g = recurrence_to_graph(&am, "UTC");
        assert_eq!(
            (g["pattern"]["dayOfMonth"].as_i64(), g["pattern"]["month"].as_i64(), g["pattern"]["interval"].as_i64()),
            (Some(31), Some(12), Some(1))
        );
    }

    #[test]
    fn recurrence_from_graph_shapes() {
        // Graph fills unused fields with defaults.
        let g = json!({
            "pattern": {"type": "weekly", "interval": 1, "month": 0, "dayOfMonth": 0, "daysOfWeek": ["monday"], "firstDayOfWeek": "sunday", "index": "first"},
            "range": {"type": "noEnd", "startDate": "2026-11-02", "endDate": "0001-01-01", "recurrenceTimeZone": "W. Europe Standard Time", "numberOfOccurrences": 0}
        });
        let r = recurrence_from_graph(&g).unwrap();
        assert!(r.end_date.is_none() && r.occurrences.is_none() && r.index.is_none());
        let mut g2 = g.clone();
        g2["range"]["type"] = json!("endDate");
        assert!(recurrence_from_graph(&g2).unwrap().end_date.is_none(), "0001-01-01 is no end date");
        g2["range"]["type"] = json!("numbered");
        assert!(recurrence_from_graph(&g2).unwrap().occurrences.is_none(), "0 occurrences is no limit");
        assert!(recurrence_from_graph(&Value::Null).is_none());
        assert!(recurrence_from_graph(&json!({})).is_none());
        assert!(recurrence_from_graph(&json!({"pattern": {}})).is_none());
        // Day order and case do not make a different series.
        let mut a = rec("weekly");
        a.days_of_week = vec!["Friday".into(), "monday".into()];
        let mut b = a.clone();
        b.days_of_week = vec!["monday".into(), "friday".into(), "monday".into()];
        assert!(same_recurrence(&a, &b));
        b.interval = 3;
        assert!(!same_recurrence(&a, &b));
    }

    #[test]
    fn auto_reply_times_in_mailbox_zone() {
        let v = json!({"status": "scheduled",
            "scheduledStartDateTime": {"dateTime": "2026-12-20T08:00:00.0000000", "timeZone": "W. Europe Standard Time"},
            "scheduledEndDateTime": {"dateTime": "2027-01-06T17:00:00.0000000", "timeZone": "UTC"}});
        let a = auto_reply_from_graph(&v);
        assert_eq!(a.scheduled_start.as_deref(), Some("2026-12-20T07:00:00Z"));
        assert_eq!(a.scheduled_end.as_deref(), Some("2027-01-06T17:00:00Z"));
        let back = auto_reply_to_graph(&a);
        assert_eq!(back["scheduledStartDateTime"], json!({"dateTime": "2026-12-20T07:00:00", "timeZone": "UTC"}));
    }

    #[test]
    fn normalize_odd_timestamps() {
        assert_eq!(normalize_utc(""), "");
        assert_eq!(normalize_utc("2026-10-15T14:40:00.1234567Z"), "2026-10-15T14:40:00Z");
        assert_eq!(normalize_utc("2026-10-15T23:40:00-05:00"), "2026-10-16T04:40:00Z");
        assert_eq!(normalize_utc("nicht-datum"), "nicht-datum");
    }
}
