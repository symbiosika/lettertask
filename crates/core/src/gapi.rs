//! Google Calendar and People API for accounts signed in with Google: a thin
//! authenticated client plus the conversions to and from the app's model
//! (the same `Calendar`, `Event` and `Contact` the Microsoft 365 side uses).
//!
//! Ids: a Google event id is only unique within its calendar, so events carry
//! `<calendarId>|<eventId>`; contacts carry `<accountId>|people/<id>`.

use chrono::{DateTime, NaiveDate, SecondsFormat, Utc};
use reqwest::Method;
use serde_json::{json, Value};

use crate::convert;
use crate::error::{Error, Result};
use crate::model::*;

pub const DEFAULT_API_BASE: &str = "https://www.googleapis.com";
pub const DEFAULT_PEOPLE_BASE: &str = "https://people.googleapis.com";

/// Read/write access to all calendars of the account.
pub const CALENDAR_SCOPE: &str = "https://www.googleapis.com/auth/calendar";
/// Read/write access to the account's contacts.
pub const CONTACTS_SCOPE: &str = "https://www.googleapis.com/auth/contacts";

/// Fields of a person read and written by the app.
pub const PERSON_FIELDS: &str = "names,emailAddresses,phoneNumbers,organizations,biographies,metadata";
const UPDATE_PERSON_FIELDS: &str = "names,emailAddresses,phoneNumbers,organizations,biographies";

/// Authenticated client for one request round of one account.
pub struct GoogleApi {
    pub http: reqwest::Client,
    /// Calendar API root, e.g. `https://www.googleapis.com`.
    pub base: String,
    /// People API root, e.g. `https://people.googleapis.com`.
    pub people_base: String,
    pub token: String,
}

pub fn seg(s: &str) -> String {
    urlencoding::encode(s).into_owned()
}

impl GoogleApi {
    fn url(&self, path: &str) -> String {
        if path.starts_with("/v1/") || path.starts_with("/v1:") {
            format!("{}{path}", self.people_base.trim_end_matches('/'))
        } else {
            format!("{}{path}", self.base.trim_end_matches('/'))
        }
    }

    pub async fn request(&self, method: Method, path: &str, body: Option<&Value>) -> Result<Value> {
        let mut req = self.http.request(method, self.url(path)).bearer_auth(&self.token);
        if let Some(b) = body {
            req = req.json(b);
        }
        let resp = req.send().await?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if status.is_success() {
            if text.trim().is_empty() {
                return Ok(Value::Null);
            }
            return serde_json::from_str(&text).map_err(|e| Error::Other(format!("Ungültige Antwort von Google: {e}")));
        }
        Err(api_error(status.as_u16(), &text))
    }

    pub async fn get(&self, path: &str) -> Result<Value> {
        self.request(Method::GET, path, None).await
    }

    /// All `items` (or `key`) of a paged list.
    pub async fn get_all(&self, path: &str, key: &str, max: usize) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let sep = if path.contains('?') { '&' } else { '?' };
            let p = match &token {
                Some(t) => format!("{path}{sep}pageToken={}", seg(t)),
                None => path.to_string(),
            };
            let v = self.get(&p).await?;
            out.extend(v[key].as_array().cloned().unwrap_or_default());
            match v["nextPageToken"].as_str() {
                Some(t) if !t.is_empty() && out.len() < max => token = Some(t.to_string()),
                _ => break,
            }
        }
        Ok(out)
    }
}

/// Google error → app error. Missing permissions get a hint what to do.
pub fn api_error(status: u16, body: &str) -> Error {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let e = &v["error"];
    let message = e["message"].as_str().unwrap_or(body).to_string();
    let reason = e["errors"][0]["reason"].as_str().unwrap_or("").to_string();
    let st = e["status"].as_str().unwrap_or("").to_string();
    let details = e["details"].to_string();
    if status == 401 {
        return Error::Auth("Die Google-Anmeldung ist abgelaufen. Bitte das Konto erneut mit Google anmelden.".into());
    }
    if status == 403 {
        if reason == "insufficientPermissions" || details.contains("ACCESS_TOKEN_SCOPE_INSUFFICIENT") || message.contains("insufficient authentication scopes") {
            return Error::Auth(
                "Für Kalender und Kontakte fehlt die Google-Berechtigung. Bitte das Konto unter Einstellungen › Konten erneut mit Google anmelden und den Zugriff auf Kalender und Kontakte erlauben.".into(),
            );
        }
        if reason == "accessNotConfigured" || details.contains("SERVICE_DISABLED") || st == "PERMISSION_DENIED" && message.contains("has not been used") {
            return Error::Auth(format!(
                "Die Google-API ist im Google-Cloud-Projekt des OAuth-Clients nicht aktiviert (Calendar API bzw. People API aktivieren). {message}"
            ));
        }
    }
    let code = if !reason.is_empty() { reason } else { st };
    Error::Graph { status, code, message }
}

/// Whether an error only means "no calendar/contacts permission (yet)".
pub fn is_permission_error(e: &Error) -> bool {
    matches!(e, Error::Auth(m) if m.contains("Google-Berechtigung") || m.contains("nicht aktiviert"))
}

fn s(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap_or("").trim().to_string()
}

// ------------------------------------------------------------------ calendars

/// `calendarList` entry → Calendar. Calendars the user does not own are
/// grouped under "Weitere Kalender".
pub fn calendar(v: &Value, account_id: &str) -> Calendar {
    let role = s(v, "accessRole");
    let primary = v["primary"].as_bool().unwrap_or(false);
    let name = Some(s(v, "summaryOverride")).filter(|n| !n.is_empty()).unwrap_or_else(|| s(v, "summary"));
    Calendar {
        id: s(v, "id"),
        account_id: account_id.to_string(),
        name,
        color: Some(s(v, "backgroundColor")).filter(|c| c.starts_with('#')),
        can_edit: matches!(role.as_str(), "owner" | "writer"),
        is_default: primary,
        group: (!primary && role != "owner").then(|| "Weitere Kalender".to_string()),
        group_id: None,
        owner: None,
    }
}

/// Event id as the app sees it.
pub fn event_id(calendar_id: &str, google_id: &str) -> String {
    format!("{calendar_id}|{google_id}")
}

/// Splits an app event id into calendar and Google event id.
pub fn split_event_id(id: &str) -> Result<(&str, &str)> {
    id.rsplit_once('|')
        .filter(|(c, e)| !c.is_empty() && !e.is_empty())
        .ok_or_else(|| Error::invalid("Ungültige Termin-ID"))
}

fn when(v: &Value) -> (String, bool) {
    if let Some(d) = v["date"].as_str() {
        return (format!("{d}T00:00:00Z"), true);
    }
    let dt = v["dateTime"].as_str().unwrap_or("");
    if let Ok(d) = DateTime::parse_from_rfc3339(dt) {
        return (d.with_timezone(&Utc).to_rfc3339_opts(SecondsFormat::Secs, true), false);
    }
    // Wall time without offset: in the given zone.
    let naive = chrono::NaiveDateTime::parse_from_str(dt, "%Y-%m-%dT%H:%M:%S%.f");
    match (naive, convert::resolve_tz(v["timeZone"].as_str().unwrap_or(""))) {
        (Ok(n), Some(tz)) => (convert::local_to_utc(n, tz).to_rfc3339_opts(SecondsFormat::Secs, true), false),
        _ => (convert::normalize_utc(dt), false),
    }
}

fn response_from_google(st: &str) -> String {
    match st {
        "accepted" => "accepted",
        "tentative" => "tentativelyAccepted",
        "declined" => "declined",
        "needsAction" => "notResponded",
        _ => "none",
    }
    .to_string()
}

fn response_to_google(r: &str) -> Option<&'static str> {
    match r {
        "accept" => Some("accepted"),
        "tentativelyAccept" => Some("tentative"),
        "decline" => Some("declined"),
        _ => None,
    }
}

fn strip_html(html: &str) -> String {
    let mut out = String::new();
    let mut tag = false;
    for ch in html.chars() {
        match ch {
            '<' => tag = true,
            '>' if tag => {
                tag = false;
                out.push(' ');
            }
            c if !tag => out.push(c),
            _ => {}
        }
    }
    let out = out.replace("&nbsp;", " ").replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">");
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Description of an event as HTML (Google stores either plain text or HTML).
pub fn body_html(v: &Value) -> String {
    let d = v["description"].as_str().unwrap_or("");
    if d.contains('<') && d.contains('>') {
        d.to_string()
    } else {
        convert::text_to_html(d)
    }
}

/// Google event → Event. `default_reminder`: the calendar's default popup
/// reminder (minutes) for events using the default reminders.
pub fn event(v: &Value, account_id: &str, calendar_id: &str, default_reminder: Option<i64>) -> Event {
    let (start, all_day) = when(&v["start"]);
    let (end, _) = when(&v["end"]);
    let organizer = (!v["organizer"].is_null()).then(|| Address {
        name: s(&v["organizer"], "displayName"),
        address: s(&v["organizer"], "email"),
    });
    let is_organizer = v["organizer"]["self"].as_bool().unwrap_or(v["organizer"].is_null());
    let atts = v["attendees"].as_array().cloned().unwrap_or_default();
    let attendees: Vec<Attendee> = atts
        .iter()
        .filter(|a| a["organizer"].as_bool() != Some(true) || !is_organizer)
        .map(|a| Attendee {
            name: s(a, "displayName"),
            address: s(a, "email"),
            kind: if a["resource"].as_bool() == Some(true) {
                "resource".into()
            } else if a["optional"].as_bool() == Some(true) {
                "optional".into()
            } else {
                "required".into()
            },
            response: response_from_google(a["responseStatus"].as_str().unwrap_or("")),
        })
        .collect();
    let me = atts.iter().find(|a| a["self"].as_bool() == Some(true));
    let response_status = if is_organizer {
        "organizer".to_string()
    } else {
        me.map(|a| response_from_google(a["responseStatus"].as_str().unwrap_or(""))).unwrap_or_else(|| "none".into())
    };
    let show_as = match (s(v, "eventType").as_str(), s(v, "transparency").as_str()) {
        ("outOfOffice", _) => "oof",
        ("workingLocation", _) => "workingElsewhere",
        (_, "transparent") => "free",
        _ if me.is_some_and(|a| a["responseStatus"].as_str() == Some("tentative")) => "tentative",
        _ => "busy",
    };
    let meet = v["hangoutLink"].as_str().map(str::to_string).or_else(|| {
        v["conferenceData"]["entryPoints"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|e| e["entryPointType"].as_str() == Some("video"))
            .and_then(|e| e["uri"].as_str().map(str::to_string))
    });
    let overrides = v["reminders"]["overrides"].as_array().cloned().unwrap_or_default();
    let use_default = v["reminders"]["useDefault"].as_bool().unwrap_or(false);
    let popup = overrides
        .iter()
        .filter(|o| o["method"].as_str() != Some("email"))
        .filter_map(|o| o["minutes"].as_i64())
        .min();
    let (is_reminder_on, reminder_minutes) = match (use_default, popup) {
        (_, Some(m)) => (true, m),
        (true, None) => (default_reminder.is_some(), default_reminder.unwrap_or(10)),
        (false, None) => (false, 15),
    };
    let rrules: Vec<String> = v["recurrence"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| r.as_str().map(str::to_string))
        .collect();
    let recurrence = rrules
        .iter()
        .find(|r| r.starts_with("RRULE:"))
        .and_then(|r| recurrence_from_rrule(r, start.get(..10).unwrap_or("")));
    let master = s(v, "recurringEventId");
    let kind = if !master.is_empty() {
        if v["originalStartTime"] != v["start"] { "exception" } else { "occurrence" }
    } else if recurrence.is_some() {
        "seriesMaster"
    } else {
        "singleInstance"
    };
    let body = body_html(v);
    let mut preview = strip_html(&body);
    if preview.chars().count() > 255 {
        preview = preview.chars().take(255).collect();
    }
    Event {
        id: event_id(calendar_id, &s(v, "id")),
        account_id: account_id.to_string(),
        calendar_id: calendar_id.to_string(),
        subject: s(v, "summary"),
        start,
        end,
        is_all_day: all_day,
        location: s(v, "location"),
        organizer,
        attendees,
        show_as: show_as.into(),
        categories: vec![],
        is_organizer,
        response_status,
        body_preview: preview,
        is_online_meeting: meet.is_some(),
        online_meeting_url: meet,
        recurrence,
        series_master_id: (!master.is_empty()).then(|| event_id(calendar_id, &master)),
        kind: kind.into(),
        is_reminder_on,
        reminder_minutes,
        importance: "normal".into(),
        sensitivity: match s(v, "visibility").as_str() {
            "private" | "confidential" => "private".into(),
            _ => "normal".into(),
        },
        is_cancelled: s(v, "status") == "cancelled",
        web_link: Some(s(v, "htmlLink")).filter(|l| !l.is_empty()),
    }
}

const RRULE_DAYS: [(&str, &str); 7] = [
    ("MO", "monday"),
    ("TU", "tuesday"),
    ("WE", "wednesday"),
    ("TH", "thursday"),
    ("FR", "friday"),
    ("SA", "saturday"),
    ("SU", "sunday"),
];
const INDEXES: [(&str, i64); 5] = [("first", 1), ("second", 2), ("third", 3), ("fourth", 4), ("last", -1)];

fn day_name(code: &str) -> Option<&'static str> {
    RRULE_DAYS.iter().find(|(c, _)| c.eq_ignore_ascii_case(code)).map(|(_, n)| *n)
}

fn day_code(name: &str) -> Option<&'static str> {
    RRULE_DAYS.iter().find(|(_, n)| n.eq_ignore_ascii_case(name)).map(|(c, _)| *c)
}

/// `RRULE:FREQ=…` → Recurrence (None for rules the app cannot show, e.g.
/// hourly ones). `start_date`: the date of the first occurrence.
pub fn recurrence_from_rrule(rule: &str, start_date: &str) -> Option<Recurrence> {
    let rule = rule.trim().strip_prefix("RRULE:").unwrap_or(rule.trim());
    let parts: std::collections::HashMap<String, String> = rule
        .split(';')
        .filter_map(|p| p.split_once('='))
        .map(|(k, v)| (k.trim().to_ascii_uppercase(), v.trim().to_string()))
        .collect();
    let start = NaiveDate::parse_from_str(start_date, "%Y-%m-%d").ok()?;
    use chrono::Datelike;
    let interval = parts.get("INTERVAL").and_then(|i| i.parse().ok()).unwrap_or(1i64).max(1);
    // BYDAY entries may carry an ordinal: "2TU", "-1FR".
    let mut ordinal: Option<i64> = None;
    let mut days = Vec::new();
    for d in parts.get("BYDAY").map(|d| d.split(',').collect::<Vec<_>>()).unwrap_or_default() {
        let d = d.trim();
        let split = d.len().saturating_sub(2);
        let (num, code) = d.split_at(split);
        if let Ok(n) = num.parse::<i64>() {
            ordinal = Some(n);
        }
        if let Some(n) = day_name(code) {
            days.push(n.to_string());
        }
    }
    if let Some(p) = parts.get("BYSETPOS").and_then(|p| p.split(',').next()?.parse::<i64>().ok()) {
        ordinal = Some(p);
    }
    let index = ordinal.map(|o| INDEXES.iter().find(|(_, n)| *n == o).map_or("last", |(i, _)| *i).to_string());
    let month_day = parts.get("BYMONTHDAY").and_then(|d| d.split(',').next()?.parse::<i64>().ok()).filter(|d| *d > 0);
    let month = parts.get("BYMONTH").and_then(|d| d.split(',').next()?.parse::<i64>().ok());
    let start_weekday = RRULE_DAYS[start.weekday().num_days_from_monday() as usize].1.to_string();
    let (pattern, days_of_week, day_of_month, month, index) = match parts.get("FREQ")?.to_ascii_uppercase().as_str() {
        "DAILY" => ("daily", vec![], 0, 0, None),
        "WEEKLY" => ("weekly", if days.is_empty() { vec![start_weekday] } else { days }, 0, 0, None),
        "MONTHLY" if !days.is_empty() && index.is_some() => ("relativeMonthly", days, 0, 0, index),
        "MONTHLY" => ("absoluteMonthly", vec![], month_day.unwrap_or(start.day() as i64), 0, None),
        "YEARLY" if !days.is_empty() && index.is_some() => {
            ("relativeYearly", days, 0, month.unwrap_or(start.month() as i64), index)
        }
        "YEARLY" => (
            "absoluteYearly",
            vec![],
            month_day.unwrap_or(start.day() as i64),
            month.unwrap_or(start.month() as i64),
            None,
        ),
        _ => return None,
    };
    let end_date = parts.get("UNTIL").and_then(|u| {
        let d = u.get(..8)?;
        NaiveDate::parse_from_str(d, "%Y%m%d").ok().map(|d| d.format("%Y-%m-%d").to_string())
    });
    Some(Recurrence {
        pattern: pattern.into(),
        interval,
        days_of_week,
        day_of_month,
        month,
        index,
        start_date: start_date.to_string(),
        end_date,
        occurrences: parts.get("COUNT").and_then(|c| c.parse().ok()),
    })
}

/// Recurrence → `RRULE:…`. `all_day`: UNTIL is a date, else the end of that day (UTC).
pub fn recurrence_to_rrule(r: &Recurrence, all_day: bool) -> String {
    let mut p = Vec::new();
    let freq = match r.pattern.as_str() {
        "daily" => "DAILY",
        "weekly" => "WEEKLY",
        "absoluteMonthly" | "relativeMonthly" => "MONTHLY",
        _ => "YEARLY",
    };
    p.push(format!("FREQ={freq}"));
    if r.interval > 1 {
        p.push(format!("INTERVAL={}", r.interval));
    }
    let days = || r.days_of_week.iter().filter_map(|d| day_code(d)).collect::<Vec<_>>().join(",");
    let setpos = || {
        let i = r.index.as_deref().filter(|i| !i.is_empty()).unwrap_or("first");
        INDEXES.iter().find(|(n, _)| *n == i).map_or(1, |(_, n)| *n)
    };
    match r.pattern.as_str() {
        "weekly" => {
            p.push(format!("BYDAY={}", days()));
            p.push("WKST=MO".into());
        }
        "absoluteMonthly" => p.push(format!("BYMONTHDAY={}", r.day_of_month.clamp(1, 31))),
        "relativeMonthly" => {
            p.push(format!("BYDAY={}", days()));
            p.push(format!("BYSETPOS={}", setpos()));
        }
        "absoluteYearly" => {
            p.push(format!("BYMONTH={}", r.month.clamp(1, 12)));
            p.push(format!("BYMONTHDAY={}", r.day_of_month.clamp(1, 31)));
        }
        "relativeYearly" => {
            p.push(format!("BYMONTH={}", r.month.clamp(1, 12)));
            p.push(format!("BYDAY={}", days()));
            p.push(format!("BYSETPOS={}", setpos()));
        }
        _ => {}
    }
    if let Some(end) = r.end_date.as_deref() {
        let d = end.replace('-', "");
        p.push(if all_day { format!("UNTIL={d}") } else { format!("UNTIL={d}T235959Z") });
    } else if let Some(n) = r.occurrences {
        p.push(format!("COUNT={n}"));
    }
    format!("RRULE:{}", p.join(";"))
}

/// Start/end of an EventInput as Google `start`/`end`.
fn when_input(ts: &str, time_zone: &str, all_day: bool) -> Value {
    let ts = ts.trim();
    if all_day {
        return json!({"date": ts.get(..10).unwrap_or(ts)});
    }
    let tz = convert::resolve_tz(time_zone).map(|t| t.name().to_string()).unwrap_or_else(|| "UTC".into());
    if let Ok(d) = DateTime::parse_from_rfc3339(ts) {
        return json!({"dateTime": d.to_rfc3339_opts(SecondsFormat::Secs, true), "timeZone": tz});
    }
    let naive = if ts.len() == 16 { format!("{ts}:00") } else { ts.get(..19).unwrap_or(ts).to_string() };
    json!({"dateTime": naive, "timeZone": tz})
}

/// EventInput → Google event JSON (insert/patch). `new_meet`: request a
/// Google Meet link (needs `conferenceDataVersion=1`).
pub fn event_input(input: &EventInput, new_meet: bool) -> Value {
    let attendees: Vec<Value> = input
        .attendees
        .iter()
        .filter(|a| !a.address.trim().is_empty())
        .map(|a| {
            let mut v = json!({"email": a.address.trim()});
            if !a.name.trim().is_empty() && a.name.trim() != a.address.trim() {
                v["displayName"] = json!(a.name.trim());
            }
            if a.kind == "optional" {
                v["optional"] = json!(true);
            }
            if a.kind == "resource" {
                v["resource"] = json!(true);
            }
            v
        })
        .collect();
    let reminders = if input.is_reminder_on {
        json!({"useDefault": false, "overrides": [{"method": "popup", "minutes": input.reminder_minutes.clamp(0, 40320)}]})
    } else {
        json!({"useDefault": false, "overrides": []})
    };
    let mut v = json!({
        "summary": input.subject,
        "location": input.location,
        "description": input.body_html,
        "start": when_input(&input.start, &input.time_zone, input.is_all_day),
        "end": when_input(&input.end, &input.time_zone, input.is_all_day),
        "attendees": attendees,
        "transparency": if input.show_as == "free" { "transparent" } else { "opaque" },
        "visibility": if matches!(input.sensitivity.as_str(), "private" | "confidential") { "private" } else { "default" },
        "reminders": reminders,
    });
    if let Some(r) = &input.recurrence {
        v["recurrence"] = json!([recurrence_to_rrule(r, input.is_all_day)]);
    }
    if new_meet {
        v["conferenceData"] = json!({"createRequest": {
            "requestId": uuid::Uuid::new_v4().to_string(),
            "conferenceSolutionKey": {"type": "hangoutsMeet"},
        }});
    }
    v
}

/// Attendee list with the own answer changed (Google has no "respond" call:
/// an attendee patches their own entry).
pub fn respond_attendees(v: &Value, response: &str, comment: &str) -> Result<Value> {
    let st = response_to_google(response).ok_or_else(|| Error::invalid("Ungültige Antwort"))?;
    let mut atts = v["attendees"].as_array().cloned().unwrap_or_default();
    let me = atts
        .iter_mut()
        .find(|a| a["self"].as_bool() == Some(true))
        .ok_or_else(|| Error::invalid("Du bist zu diesem Termin nicht eingeladen."))?;
    me["responseStatus"] = json!(st);
    if !comment.trim().is_empty() {
        me["comment"] = json!(comment.trim());
    }
    Ok(Value::Array(atts))
}

// ------------------------------------------------------------------ contacts

/// Contact ids are global: `<accountId>|people/<id>`.
pub fn contact_id(account_id: &str, resource: &str) -> String {
    format!("{account_id}|{resource}")
}

pub fn person_resource(id: &str) -> Option<&str> {
    id.split_once('|').map(|(_, r)| r).filter(|r| r.starts_with("people/"))
}

fn first<'a>(v: &'a Value, key: &str) -> &'a Value {
    let list = v[key].as_array();
    list.and_then(|l| l.iter().find(|x| x["metadata"]["primary"].as_bool() == Some(true)).or_else(|| l.first()))
        .unwrap_or(&Value::Null)
}

pub fn contact(v: &Value, account_id: &str) -> Contact {
    let name = first(v, "names");
    let org = first(v, "organizations");
    let emails = v["emailAddresses"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|e| s(e, "value"))
        .filter(|e| !e.is_empty())
        .collect::<Vec<_>>();
    let phones = v["phoneNumbers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|p| !s(p, "value").is_empty())
        .map(|p| Phone {
            kind: match s(p, "type").to_ascii_lowercase().as_str() {
                "mobile" => "mobile",
                "home" | "homefax" => "home",
                _ => "business",
            }
            .into(),
            number: s(p, "value"),
        })
        .collect();
    let mut display = s(name, "displayName");
    if display.is_empty() {
        display = format!("{} {}", s(name, "givenName"), s(name, "familyName")).trim().to_string();
    }
    if display.is_empty() {
        display = emails.first().cloned().unwrap_or_default();
    }
    Contact {
        id: contact_id(account_id, &s(v, "resourceName")),
        account_id: account_id.to_string(),
        source: "google".into(),
        display_name: display,
        given_name: s(name, "givenName"),
        surname: s(name, "familyName"),
        emails,
        company: s(org, "name"),
        job_title: s(org, "title"),
        phones,
        notes: s(first(v, "biographies"), "value"),
        read_only: false,
    }
}

pub fn person_input(c: &ContactInput, etag: Option<&str>) -> Value {
    let mut name = json!({"givenName": c.given_name.trim(), "familyName": c.surname.trim()});
    if c.given_name.trim().is_empty() && c.surname.trim().is_empty() {
        name["unstructuredName"] = json!(c.display_name.trim());
    }
    let mut v = json!({
        "names": [name],
        "emailAddresses": c.emails.iter().map(|e| e.trim()).filter(|e| !e.is_empty())
            .map(|e| json!({"value": e})).collect::<Vec<_>>(),
        "phoneNumbers": c.phones.iter().filter(|p| !p.number.trim().is_empty())
            .map(|p| json!({"value": p.number.trim(), "type": match p.kind.as_str() { "mobile" => "mobile", "home" => "home", _ => "work" }}))
            .collect::<Vec<_>>(),
        "organizations": if c.company.trim().is_empty() && c.job_title.trim().is_empty() { json!([]) }
            else { json!([{"name": c.company.trim(), "title": c.job_title.trim()}]) },
        "biographies": if c.notes.trim().is_empty() { json!([]) } else { json!([{"value": c.notes, "contentType": "TEXT_PLAIN"}]) },
    });
    if let Some(e) = etag {
        v["etag"] = json!(e);
    }
    v
}

pub fn update_person_fields() -> &'static str {
    UPDATE_PERSON_FIELDS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timed_event_is_converted_to_utc() {
        let v = json!({
            "id": "abc", "summary": "Jour fixe", "status": "confirmed",
            "start": {"dateTime": "2026-10-26T10:00:00+01:00", "timeZone": "Europe/Berlin"},
            "end": {"dateTime": "2026-10-26T11:00:00+01:00"},
            "organizer": {"email": "me@gmail.com", "self": true},
            "attendees": [
                {"email": "me@gmail.com", "self": true, "organizer": true, "responseStatus": "accepted"},
                {"email": "a@x.de", "displayName": "Anna", "responseStatus": "tentative", "optional": true}
            ],
            "hangoutLink": "https://meet.google.com/abc-defg-hij",
            "reminders": {"useDefault": true},
            "description": "Agenda <b>wichtig</b>",
            "htmlLink": "https://calendar.google.com/x"
        });
        let e = event(&v, "imap:me@gmail.com", "me@gmail.com", Some(30));
        assert_eq!(e.id, "me@gmail.com|abc");
        assert_eq!(e.start, "2026-10-26T09:00:00Z");
        assert_eq!(e.end, "2026-10-26T10:00:00Z");
        assert!(!e.is_all_day && e.is_organizer);
        assert_eq!(e.response_status, "organizer");
        assert_eq!(e.attendees.len(), 1);
        assert_eq!(e.attendees[0].kind, "optional");
        assert_eq!(e.attendees[0].response, "tentativelyAccepted");
        assert_eq!(e.online_meeting_url.as_deref(), Some("https://meet.google.com/abc-defg-hij"));
        assert!(e.is_reminder_on);
        assert_eq!(e.reminder_minutes, 30);
        assert_eq!(e.body_preview, "Agenda wichtig");
        assert_eq!(e.kind, "singleInstance");
    }

    #[test]
    fn all_day_and_invitation() {
        let v = json!({
            "id": "x_20261225", "recurringEventId": "x",
            "start": {"date": "2026-12-25"}, "end": {"date": "2026-12-26"},
            "originalStartTime": {"date": "2026-12-25"},
            "organizer": {"email": "boss@firma.de"},
            "attendees": [{"email": "me@gmail.com", "self": true, "responseStatus": "needsAction"}],
            "transparency": "transparent"
        });
        let e = event(&v, "acc", "cal", None);
        assert!(e.is_all_day);
        assert_eq!(e.start, "2026-12-25T00:00:00Z");
        assert_eq!(e.end, "2026-12-26T00:00:00Z");
        assert!(!e.is_organizer);
        assert_eq!(e.response_status, "notResponded");
        assert_eq!(e.show_as, "free");
        assert_eq!(e.kind, "occurrence");
        assert_eq!(e.series_master_id.as_deref(), Some("cal|x"));
        assert!(!e.is_reminder_on);
    }

    #[test]
    fn rrule_round_trips() {
        let cases = [
            ("RRULE:FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE;WKST=MO;COUNT=10", "weekly"),
            ("RRULE:FREQ=MONTHLY;BYDAY=TU;BYSETPOS=-1;UNTIL=20271231T235959Z", "relativeMonthly"),
            ("RRULE:FREQ=MONTHLY;BYMONTHDAY=15", "absoluteMonthly"),
            ("RRULE:FREQ=YEARLY;BYMONTH=3;BYDAY=SU;BYSETPOS=2", "relativeYearly"),
            ("RRULE:FREQ=YEARLY;BYMONTH=12;BYMONTHDAY=24", "absoluteYearly"),
            ("RRULE:FREQ=DAILY", "daily"),
        ];
        for (rule, pattern) in cases {
            let r = recurrence_from_rrule(rule, "2026-10-26").unwrap();
            assert_eq!(r.pattern, pattern, "{rule}");
            let back = recurrence_from_rrule(&recurrence_to_rrule(&r, false), "2026-10-26").unwrap();
            assert!(convert::same_recurrence(&r, &back), "{rule}: {r:?} vs {back:?}");
        }
        // Ordinal inside BYDAY ("2TU") and an all-day UNTIL.
        let r = recurrence_from_rrule("RRULE:FREQ=MONTHLY;BYDAY=2TU;UNTIL=20270101", "2026-10-13").unwrap();
        assert_eq!(r.pattern, "relativeMonthly");
        assert_eq!(r.index.as_deref(), Some("second"));
        assert_eq!(r.end_date.as_deref(), Some("2027-01-01"));
        assert!(recurrence_to_rrule(&r, true).ends_with("UNTIL=20270101"));
        // Weekly without BYDAY repeats on the start's weekday (2026-10-26 is a Monday).
        assert_eq!(recurrence_from_rrule("RRULE:FREQ=WEEKLY", "2026-10-26").unwrap().days_of_week, ["monday"]);
        assert!(recurrence_from_rrule("RRULE:FREQ=HOURLY", "2026-10-26").is_none());
    }

    #[test]
    fn input_is_converted() {
        let input = EventInput {
            subject: "Termin".into(),
            start: "2026-10-26T10:00".into(),
            end: "2026-10-26T11:00:00".into(),
            time_zone: "W. Europe Standard Time".into(),
            is_all_day: false,
            location: "Büro".into(),
            body_html: "<p>Hallo</p>".into(),
            attendees: vec![AttendeeInput { name: "".into(), address: "a@x.de".into(), kind: "optional".into() }],
            show_as: "free".into(),
            categories: vec![],
            is_reminder_on: true,
            reminder_minutes: 15,
            is_online_meeting: true,
            recurrence: None,
            sensitivity: "private".into(),
            importance: "normal".into(),
        };
        let v = event_input(&input, true);
        assert_eq!(v["start"], json!({"dateTime": "2026-10-26T10:00:00", "timeZone": "Europe/Berlin"}));
        assert_eq!(v["end"]["dateTime"], "2026-10-26T11:00:00");
        assert_eq!(v["attendees"][0], json!({"email": "a@x.de", "optional": true}));
        assert_eq!(v["transparency"], "transparent");
        assert_eq!(v["visibility"], "private");
        assert_eq!(v["reminders"]["overrides"][0]["minutes"], 15);
        assert_eq!(v["conferenceData"]["createRequest"]["conferenceSolutionKey"]["type"], "hangoutsMeet");
        assert!(v.get("recurrence").is_none());
        let all_day = EventInput { is_all_day: true, start: "2026-12-24T00:00:00".into(), end: "2026-12-25T00:00:00".into(), ..input };
        let v = event_input(&all_day, false);
        assert_eq!(v["start"], json!({"date": "2026-12-24"}));
        assert_eq!(v["end"], json!({"date": "2026-12-25"}));
        assert!(v.get("conferenceData").is_none());
    }

    #[test]
    fn respond_changes_only_own_entry() {
        let v = json!({"attendees": [{"email": "a@x.de", "responseStatus": "accepted"}, {"email": "me@gmail.com", "self": true, "responseStatus": "needsAction"}]});
        let a = respond_attendees(&v, "tentativelyAccept", "komme später").unwrap();
        assert_eq!(a[0]["responseStatus"], "accepted");
        assert_eq!(a[1]["responseStatus"], "tentative");
        assert_eq!(a[1]["comment"], "komme später");
        assert!(respond_attendees(&json!({"attendees": []}), "accept", "").is_err());
        assert!(respond_attendees(&v, "maybe", "").is_err());
    }

    #[test]
    fn contacts_convert_both_ways() {
        let v = json!({
            "resourceName": "people/c1", "etag": "e1",
            "names": [{"displayName": "Max Muster", "givenName": "Max", "familyName": "Muster", "metadata": {"primary": true}}],
            "emailAddresses": [{"value": "max@x.de"}, {"value": "m@y.de"}],
            "phoneNumbers": [{"value": "+49 1", "type": "mobile"}, {"value": "+49 2", "type": "work"}],
            "organizations": [{"name": "ACME", "title": "CTO"}],
            "biographies": [{"value": "Notiz"}]
        });
        let c = contact(&v, "imap:me@gmail.com");
        assert_eq!(c.id, "imap:me@gmail.com|people/c1");
        assert_eq!(person_resource(&c.id), Some("people/c1"));
        assert_eq!(c.source, "google");
        assert_eq!(c.display_name, "Max Muster");
        assert_eq!(c.emails, ["max@x.de", "m@y.de"]);
        assert_eq!(c.phones[0].kind, "mobile");
        assert_eq!(c.phones[1].kind, "business");
        assert_eq!((c.company.as_str(), c.job_title.as_str(), c.notes.as_str()), ("ACME", "CTO", "Notiz"));
        let input = ContactInput {
            display_name: c.display_name.clone(),
            given_name: c.given_name.clone(),
            surname: c.surname.clone(),
            emails: c.emails.clone(),
            company: c.company.clone(),
            job_title: c.job_title.clone(),
            phones: c.phones.clone(),
            notes: c.notes.clone(),
        };
        let p = person_input(&input, Some("e1"));
        assert_eq!(p["etag"], "e1");
        assert_eq!(p["phoneNumbers"][1]["type"], "work");
        let back = contact(&json!({"resourceName": "people/c1", "names": p["names"], "emailAddresses": p["emailAddresses"],
            "phoneNumbers": p["phoneNumbers"], "organizations": p["organizations"], "biographies": p["biographies"]}), "imap:me@gmail.com");
        assert_eq!(back.emails, c.emails);
        assert_eq!(back.phones, c.phones);
        assert_eq!(back.company, "ACME");
    }

    #[test]
    fn permission_errors_get_a_hint() {
        let e = api_error(403, r#"{"error":{"code":403,"message":"Request had insufficient authentication scopes.","status":"PERMISSION_DENIED","details":[{"reason":"ACCESS_TOKEN_SCOPE_INSUFFICIENT"}]}}"#);
        assert!(is_permission_error(&e), "{e}");
        let e = api_error(403, r#"{"error":{"code":403,"message":"Google Calendar API has not been used in project 1 before or it is disabled.","errors":[{"reason":"accessNotConfigured"}]}}"#);
        assert!(is_permission_error(&e) && e.to_string().contains("Calendar API"), "{e}");
        assert!(api_error(404, r#"{"error":{"code":404,"message":"Not Found"}}"#).is_not_found());
    }
}
