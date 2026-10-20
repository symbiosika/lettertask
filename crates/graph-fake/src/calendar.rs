//! Calendars, events, calendarView with recurrence expansion, meeting responses.

use std::collections::{HashMap, HashSet};
use std::str::FromStr;

use axum::http::{Method, StatusCode};
use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, TimeZone, Utc, Weekday};
use chrono_tz::Tz;
use serde_json::{json, Value};

use crate::state::{Ev, Mailbox, Occ};
use crate::util::{apply_select, created, graph_dt, iso, new_id, no_content, ok, paged, parse_naive, parse_utc, preview, GErr, GResult, Req};

const OCC_SEP: &str = "_OCC_";

pub fn handle(mb: &mut Mailbox, rest: &[&str], req: &Req) -> GResult {
    let m = &req.method;
    let default_cal = mb.default_calendar_id();
    match rest {
        ["calendars"] if m == Method::GET => {
            let vals: Vec<Value> = mb.calendars.iter().map(|c| c.render(&mb.display_name, &mb.addr)).collect();
            ok(paged(req, vals, 10))
        }
        ["calendars", cid] if m == Method::GET => {
            let c = mb.calendars.iter().find(|c| c.id == *cid).ok_or_else(|| GErr::not_found("calendar"))?;
            ok(c.render(&mb.display_name, &mb.addr))
        }
        ["calendar"] if m == Method::GET => {
            let c = mb.calendars.iter().find(|c| c.is_default).ok_or_else(|| GErr::not_found("calendar"))?;
            ok(c.render(&mb.display_name, &mb.addr))
        }
        ["calendars", cid, "calendarView"] if m == Method::GET => calendar_view(mb, &cal(mb, cid)?, req),
        ["calendar", "calendarView"] | ["calendarView"] if m == Method::GET => calendar_view(mb, &default_cal, req),
        ["calendars", cid, "events"] => events_collection(mb, &cal(mb, cid)?, req),
        ["calendar", "events"] | ["events"] => events_collection(mb, &default_cal, req),
        ["events", id] => event_item(mb, id, req),
        ["events", id, "instances"] if m == Method::GET => instances(mb, id, req),
        ["events", id, "calendar"] if m == Method::GET => {
            let master = split_occ(id).map_or(*id, |(m, _)| m);
            let ev = mb.events.get(master).ok_or_else(|| GErr::not_found("event"))?;
            let c = mb.calendars.iter().find(|c| c.id == ev.cal_id).ok_or_else(|| GErr::not_found("calendar"))?;
            ok(apply_select(c.render(&mb.display_name, &mb.addr), req.q("select")))
        }
        ["events", id, action] if m == Method::POST => event_action(mb, id, action, req),
        _ => Err(GErr::new(StatusCode::BAD_REQUEST, "BadRequest", format!("Unsupported request: {} /{}", req.method, rest.join("/")))),
    }
}

fn cal(mb: &Mailbox, id: &str) -> Result<String, GErr> {
    mb.calendars.iter().find(|c| c.id == id).map(|c| c.id.clone()).ok_or_else(|| GErr::not_found("calendar"))
}

fn ensure_editable(mb: &Mailbox, cal_id: &str) -> Result<(), GErr> {
    match mb.calendars.iter().find(|c| c.id == cal_id) {
        Some(c) if c.can_edit => Ok(()),
        Some(_) => Err(GErr::access_denied()),
        None => Err(GErr::not_found("calendar")),
    }
}

// ---------------------------------------------------------------- time zones

pub fn parse_tz(name: &str) -> Option<Tz> {
    let n = name.trim();
    let mapped = match n {
        "" | "UTC" | "utc" | "Etc/UTC" | "Coordinated Universal Time" | "tzone://Microsoft/Utc" => "UTC",
        "W. Europe Standard Time" => "Europe/Berlin",
        // Exchange labels zones from foreign ICS invites like this; clients
        // cannot resolve it. The fake treats it as W. Europe internally.
        "Customized Time Zone" => "Europe/Berlin",
        "Central Europe Standard Time" => "Europe/Budapest",
        "Romance Standard Time" => "Europe/Paris",
        "GMT Standard Time" => "Europe/London",
        "Eastern Standard Time" => "America/New_York",
        "Central Standard Time" => "America/Chicago",
        "Pacific Standard Time" => "America/Los_Angeles",
        other => other,
    };
    Tz::from_str(mapped).ok()
}

pub fn to_utc(local: NaiveDateTime, tz: Tz) -> NaiveDateTime {
    tz.from_local_datetime(&local)
        .earliest()
        .or_else(|| tz.from_local_datetime(&(local + Duration::hours(1))).earliest())
        .map(|d| d.naive_utc())
        .unwrap_or(local)
}

pub fn from_utc(utc: NaiveDateTime, tz: Tz) -> NaiveDateTime {
    tz.from_utc_datetime(&utc).naive_local()
}

/// Parses `{dateTime, timeZone}`; returns (local naive, tz name, tz).
fn parse_dt_tz(v: &Value, field: &str) -> Result<(NaiveDateTime, String, Tz), GErr> {
    let dt = v["dateTime"].as_str().and_then(parse_naive).ok_or_else(|| GErr::bad(format!("{field}.dateTime is missing or invalid.")))?;
    let tz_name = v["timeZone"].as_str().unwrap_or("UTC").to_string();
    let tz = parse_tz(&tz_name).ok_or_else(|| {
        GErr::new(StatusCode::BAD_REQUEST, "TimeZoneNotSupportedException", format!("A valid TimeZone value must be specified. The following TimeZone value is not supported: '{tz_name}'."))
    })?;
    Ok((dt, tz_name, tz))
}

// ---------------------------------------------------------------- recurrence

fn weekday(s: &str) -> Option<Weekday> {
    Some(match s.to_ascii_lowercase().as_str() {
        "monday" => Weekday::Mon,
        "tuesday" => Weekday::Tue,
        "wednesday" => Weekday::Wed,
        "thursday" => Weekday::Thu,
        "friday" => Weekday::Fri,
        "saturday" => Weekday::Sat,
        "sunday" => Weekday::Sun,
        _ => return None,
    })
}

fn add_months(y: i32, m: u32, k: i64) -> (i32, u32) {
    let total = y as i64 * 12 + (m as i64 - 1) + k;
    ((total / 12) as i32, (total % 12) as u32 + 1)
}

fn nth_weekday(y: i32, m: u32, dows: &[Weekday], index: &str) -> Option<NaiveDate> {
    let mut days = Vec::new();
    let mut d = NaiveDate::from_ymd_opt(y, m, 1)?;
    while d.month() == m {
        if dows.contains(&d.weekday()) {
            days.push(d);
        }
        d = d.succ_opt()?;
    }
    let i = match index {
        "second" => 1,
        "third" => 2,
        "fourth" => 3,
        "last" => return days.last().copied(),
        _ => 0,
    };
    days.get(i).copied()
}

/// All occurrence dates of a recurrence up to `until` (inclusive), honouring range end / count.
fn pattern_dates(rec: &Value, until: NaiveDate) -> Vec<NaiveDate> {
    let p = &rec["pattern"];
    let r = &rec["range"];
    let interval = p["interval"].as_i64().unwrap_or(1).max(1);
    let Some(start) = r["startDate"].as_str().and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()) else {
        return vec![];
    };
    let range_type = r["type"].as_str().unwrap_or("noEnd");
    let end_date = if range_type == "endDate" { r["endDate"].as_str().and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()) } else { None };
    let max_count = if range_type == "numbered" { r["numberOfOccurrences"].as_u64().map(|n| n as usize) } else { None };
    let limit = end_date.map_or(until, |e| e.min(until));
    let dows: Vec<Weekday> = p["daysOfWeek"].as_array().into_iter().flatten().filter_map(|d| d.as_str().and_then(weekday)).collect();
    let dom = p["dayOfMonth"].as_u64().unwrap_or(1) as u32;
    let month = p["month"].as_u64().unwrap_or(1) as u32;
    let index = p["index"].as_str().unwrap_or("first");

    let mut out = Vec::new();
    // Count-limited series must be generated from the start even past `until`.
    let gen_limit = if max_count.is_some() { end_date.unwrap_or(NaiveDate::MAX) } else { limit };
    let push = |d: NaiveDate, out: &mut Vec<NaiveDate>| -> bool {
        if max_count.is_some_and(|m| out.len() >= m) || d > gen_limit {
            return false;
        }
        if d >= start {
            out.push(d);
        }
        true
    };
    let mut guard = 0;
    match p["type"].as_str().unwrap_or("") {
        "daily" => {
            let mut d = start;
            while d <= limit && guard < 100_000 && push(d, &mut out) {
                d += Duration::days(interval);
                guard += 1;
            }
        }
        "weekly" => {
            let first = p["firstDayOfWeek"].as_str().and_then(weekday).unwrap_or(Weekday::Sun);
            let offset = (start.weekday().num_days_from_monday() + 7 - first.num_days_from_monday()) % 7;
            let mut week = start - Duration::days(offset as i64);
            'outer: while week <= limit && guard < 100_000 {
                for i in 0..7 {
                    let d = week + Duration::days(i);
                    if d > limit {
                        break 'outer;
                    }
                    if dows.contains(&d.weekday()) && d >= start && !push(d, &mut out) {
                        break 'outer;
                    }
                }
                week += Duration::days(7 * interval);
                guard += 1;
            }
        }
        "absoluteMonthly" | "relativeMonthly" | "absoluteYearly" | "relativeYearly" => {
            let yearly = p["type"].as_str().unwrap_or("").ends_with("Yearly");
            let step = if yearly { 12 * interval } else { interval };
            let (y0, m0) = if yearly { (start.year(), month) } else { (start.year(), start.month()) };
            let mut k = 0i64;
            while guard < 100_000 {
                let (y, m) = add_months(y0, m0, k * step);
                let Some(first_of_month) = NaiveDate::from_ymd_opt(y, m, 1) else { break };
                if first_of_month > limit {
                    break;
                }
                let d = if p["type"].as_str().unwrap_or("").starts_with("absolute") {
                    // Like Exchange: day 29–31 falls on the last day of shorter months.
                    (1..=dom.max(1)).rev().find_map(|day| NaiveDate::from_ymd_opt(y, m, day))
                } else {
                    nth_weekday(y, m, &dows, index)
                };
                if let Some(d) = d {
                    if d <= limit && !push(d, &mut out) {
                        break;
                    }
                }
                k += 1;
                guard += 1;
            }
        }
        _ => {
            push(start, &mut out);
        }
    }
    out.retain(|d| *d <= limit);
    out
}

fn normalize_recurrence(r: &Value, tz_label: &str) -> Result<Value, GErr> {
    if r.is_null() {
        return Ok(Value::Null);
    }
    let p = &r["pattern"];
    let ty = p["type"].as_str().unwrap_or("");
    if !["daily", "weekly", "absoluteMonthly", "relativeMonthly", "absoluteYearly", "relativeYearly"].contains(&ty) {
        return Err(GErr::bad(format!("recurrence.pattern.type '{ty}' is invalid.")));
    }
    let days = p["daysOfWeek"].as_array().cloned().unwrap_or_default();
    if (ty == "weekly" || ty.starts_with("relative")) && days.is_empty() {
        return Err(GErr::bad("recurrence.pattern.daysOfWeek is required for this pattern type."));
    }
    if ty.starts_with("absolute") && ty != "daily" && p["dayOfMonth"].as_u64().unwrap_or(0) == 0 {
        return Err(GErr::bad("recurrence.pattern.dayOfMonth is required for this pattern type."));
    }
    if ty.ends_with("Yearly") && p["month"].as_u64().unwrap_or(0) == 0 {
        return Err(GErr::bad("recurrence.pattern.month is required for yearly patterns."));
    }
    let rg = &r["range"];
    let rty = rg["type"].as_str().unwrap_or("");
    if !["endDate", "noEnd", "numbered"].contains(&rty) {
        return Err(GErr::bad(format!("recurrence.range.type '{rty}' is invalid.")));
    }
    let start = rg["startDate"].as_str().filter(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok());
    let Some(start) = start else {
        return Err(GErr::bad("recurrence.range.startDate is required (YYYY-MM-DD)."));
    };
    if rty == "endDate" && rg["endDate"].as_str().and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()).is_none() {
        return Err(GErr::bad("recurrence.range.endDate is required for range type endDate."));
    }
    if rty == "numbered" && rg["numberOfOccurrences"].as_u64().unwrap_or(0) == 0 {
        return Err(GErr::bad("recurrence.range.numberOfOccurrences must be > 0."));
    }
    Ok(json!({
        "pattern": {
            "type": ty,
            "interval": p["interval"].as_u64().unwrap_or(1).max(1),
            "month": p["month"].as_u64().unwrap_or(0),
            "dayOfMonth": p["dayOfMonth"].as_u64().unwrap_or(0),
            "daysOfWeek": days,
            "firstDayOfWeek": p["firstDayOfWeek"].as_str().unwrap_or("sunday"),
            "index": p["index"].as_str().unwrap_or("first"),
        },
        "range": {
            "type": rty,
            "startDate": start,
            "endDate": if rty == "endDate" { rg["endDate"].clone() } else { json!("0001-01-01") },
            "recurrenceTimeZone": rg["recurrenceTimeZone"].as_str().unwrap_or(tz_label),
            "numberOfOccurrences": if rty == "numbered" { rg["numberOfOccurrences"].clone() } else { json!(0) },
        }
    }))
}

// ---------------------------------------------------------------- rendering

pub struct OccRef<'a> {
    pub key: String,
    pub start: NaiveDateTime,
    pub end: NaiveDateTime,
    pub patch: Option<&'a Value>,
}

fn occ_id(master: &str, key: &str) -> String {
    format!("{master}{OCC_SEP}{key}")
}

fn out_dt(n: NaiveDateTime, all_day: bool, out_tz: &str) -> Value {
    let tz = parse_tz(out_tz).unwrap_or(Tz::UTC);
    let label = if parse_tz(out_tz).is_some() { out_tz } else { "UTC" };
    let local = if all_day { n } else { from_utc(n, tz) };
    json!({ "dateTime": graph_dt(local), "timeZone": label })
}

/// Renders an event (or one occurrence of a series) with times in `out_tz`.
pub fn render_event(ev: &Ev, occ: Option<OccRef<'_>>, out_tz: &str) -> Value {
    let mut v = ev.json.clone();
    v["isAllDay"] = json!(ev.all_day);
    match occ {
        None => {
            v["id"] = json!(ev.id);
            v["start"] = out_dt(ev.start, ev.all_day, out_tz);
            v["end"] = out_dt(ev.end, ev.all_day, out_tz);
            v["type"] = json!(if ev.recurrence.is_some() { "seriesMaster" } else { "singleInstance" });
            v["seriesMasterId"] = Value::Null;
            v["recurrence"] = ev.recurrence.clone().unwrap_or(Value::Null);
        }
        Some(o) => {
            if let Some(p) = o.patch {
                crate::util::merge(&mut v, p);
            }
            v["id"] = json!(occ_id(&ev.id, &o.key));
            v["start"] = out_dt(o.start, ev.all_day, out_tz);
            v["end"] = out_dt(o.end, ev.all_day, out_tz);
            v["type"] = json!(if o.patch.is_some() { "exception" } else { "occurrence" });
            v["seriesMasterId"] = json!(ev.id);
            v["recurrence"] = Value::Null;
            let orig = NaiveDate::parse_from_str(&o.key, "%Y%m%d").ok().map(|d| d.and_time(local_start_time(ev)));
            if let Some(orig) = orig {
                let utc = if ev.all_day { orig } else { to_utc(orig, ev_tz(ev)) };
                v["originalStart"] = json!(iso(utc.and_utc()));
            }
        }
    }
    v["@odata.etag"] = json!(format!("W/\"{}\"", v["changeKey"].as_str().unwrap_or("")));
    v
}

fn ev_tz(ev: &Ev) -> Tz {
    parse_tz(&ev.tz).unwrap_or(Tz::UTC)
}

fn local_start_time(ev: &Ev) -> chrono::NaiveTime {
    if ev.all_day {
        ev.start.time()
    } else {
        from_utc(ev.start, ev_tz(ev)).time()
    }
}

/// The occurrence of series `ev` on original local date `key`.
fn occurrence_at<'a>(ev: &'a Ev, key: &str) -> Option<OccRef<'a>> {
    if ev.deleted_occ.contains(key) {
        return None;
    }
    if let Some(o) = ev.exceptions.get(key) {
        return Some(OccRef { key: key.to_string(), start: o.start, end: o.end, patch: Some(&o.patch) });
    }
    let d = NaiveDate::parse_from_str(key, "%Y%m%d").ok()?;
    let rec = ev.recurrence.as_ref()?;
    if !pattern_dates(rec, d).contains(&d) {
        return None;
    }
    let local = d.and_time(local_start_time(ev));
    let start = if ev.all_day { local } else { to_utc(local, ev_tz(ev)) };
    Some(OccRef { key: key.to_string(), start, end: start + (ev.end - ev.start), patch: None })
}

/// Expands a series into occurrences overlapping [from, to) (UTC).
fn expand<'a>(ev: &'a Ev, from: NaiveDateTime, to: NaiveDateTime, view_tz: Tz) -> Vec<OccRef<'a>> {
    let Some(rec) = &ev.recurrence else { return vec![] };
    let tz = ev_tz(ev);
    let until = if ev.all_day { from_utc(to, view_tz).date() } else { from_utc(to, tz).date() } + Duration::days(1);
    let time = local_start_time(ev);
    let dur = ev.end - ev.start;
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for d in pattern_dates(rec, until) {
        let key = d.format("%Y%m%d").to_string();
        if ev.deleted_occ.contains(&key) {
            continue;
        }
        seen.insert(key.clone());
        let (start, end, patch) = match ev.exceptions.get(&key) {
            Some(o) => (o.start, o.end, Some(&o.patch)),
            None => {
                let local = d.and_time(time);
                let s = if ev.all_day { local } else { to_utc(local, tz) };
                (s, s + dur, None)
            }
        };
        if overlaps(start, end, ev.all_day, from, to, view_tz) {
            out.push(OccRef { key, start, end, patch });
        }
    }
    // exceptions moved away from their original date window
    for (key, o) in &ev.exceptions {
        if !seen.contains(key) && !ev.deleted_occ.contains(key) && overlaps(o.start, o.end, ev.all_day, from, to, view_tz) {
            out.push(OccRef { key: key.clone(), start: o.start, end: o.end, patch: Some(&o.patch) });
        }
    }
    out
}

fn overlaps(start: NaiveDateTime, end: NaiveDateTime, all_day: bool, from: NaiveDateTime, to: NaiveDateTime, view_tz: Tz) -> bool {
    let (s, e) = if all_day { (to_utc(start, view_tz), to_utc(end, view_tz)) } else { (start, end) };
    s < to && (e > from || (e == s && s >= from))
}

fn sort_key(v: &Value) -> String {
    v.pointer("/start/dateTime").and_then(Value::as_str).unwrap_or("").to_string()
}

fn view_window(req: &Req) -> Result<(NaiveDateTime, NaiveDateTime), GErr> {
    let missing = || {
        GErr::new(
            StatusCode::BAD_REQUEST,
            "ErrorInvalidParameter",
            "This request requires a time window specified by the query string parameters StartDateTime and EndDateTime.",
        )
    };
    let from = req.q("startDateTime").and_then(parse_utc).ok_or_else(missing)?;
    let to = req.q("endDateTime").and_then(parse_utc).ok_or_else(missing)?;
    Ok((from.naive_utc(), to.naive_utc()))
}

fn out_tz(req: &Req) -> String {
    req.prefer_tz().filter(|t| parse_tz(t).is_some()).unwrap_or_else(|| "UTC".to_string())
}

fn calendar_view(mb: &Mailbox, cal_id: &str, req: &Req) -> GResult {
    let (from, to) = view_window(req)?;
    let tz_name = out_tz(req);
    let view_tz = parse_tz(&tz_name).unwrap_or(Tz::UTC);
    let mut vals = Vec::new();
    for ev in mb.events.values().filter(|e| e.cal_id == cal_id) {
        if ev.recurrence.is_some() {
            for o in expand(ev, from, to, view_tz) {
                vals.push(render_event(ev, Some(o), &tz_name));
            }
        } else if overlaps(ev.start, ev.end, ev.all_day, from, to, view_tz) {
            vals.push(render_event(ev, None, &tz_name));
        }
    }
    vals.sort_by_key(sort_key);
    let vals = vals.into_iter().map(|v| apply_select(v, req.q("select"))).collect();
    ok(paged(req, vals, 10))
}

fn instances(mb: &Mailbox, id: &str, req: &Req) -> GResult {
    let ev = mb.events.get(id).ok_or_else(|| GErr::not_found("event"))?;
    let (from, to) = view_window(req)?;
    let tz_name = out_tz(req);
    let view_tz = parse_tz(&tz_name).unwrap_or(Tz::UTC);
    let mut vals: Vec<Value> = expand(ev, from, to, view_tz).into_iter().map(|o| render_event(ev, Some(o), &tz_name)).collect();
    vals.sort_by_key(sort_key);
    ok(paged(req, vals, 10))
}

// ---------------------------------------------------------------- CRUD

fn attendees(v: &Value) -> Value {
    json!(v
        .as_array()
        .into_iter()
        .flatten()
        .map(|a| json!({
            "type": a["type"].as_str().unwrap_or("required"),
            "status": a.get("status").cloned().unwrap_or(json!({ "response": "none", "time": "0001-01-01T00:00:00Z" })),
            "emailAddress": {
                "name": a.pointer("/emailAddress/name").and_then(Value::as_str).unwrap_or(""),
                "address": a.pointer("/emailAddress/address").and_then(Value::as_str).unwrap_or(""),
            }
        }))
        .collect::<Vec<_>>())
}

fn join_url() -> String {
    format!(
        "https://teams.microsoft.com/l/meetup-join/19%3ameeting_{}%40thread.v2/0",
        uuid::Uuid::new_v4().simple()
    )
}

/// Applies writable properties from `body` onto event json (not times).
fn apply_props(j: &mut Value, body: &Value) {
    for k in ["subject", "showAs", "categories", "isReminderOn", "reminderMinutesBeforeStart", "importance", "sensitivity", "responseRequested", "allowNewTimeProposals", "hideAttendees"] {
        if let Some(v) = body.get(k) {
            j[k] = v.clone();
        }
    }
    if let Some(b) = body.get("body") {
        j["body"] = json!({
            "contentType": b["contentType"].as_str().unwrap_or("html"),
            "content": b["content"].as_str().unwrap_or(""),
        });
        j["bodyPreview"] = json!(preview(&j["body"]));
    }
    if let Some(l) = body.get("location") {
        j["location"] = json!({
            "displayName": l["displayName"].as_str().unwrap_or(""),
            "locationType": l["locationType"].as_str().unwrap_or("default"),
            "uniqueIdType": "unknown",
        });
    }
    if let Some(a) = body.get("attendees") {
        j["attendees"] = attendees(a);
    }
    if let Some(om) = body.get("isOnlineMeeting").and_then(Value::as_bool) {
        j["isOnlineMeeting"] = json!(om);
        if om {
            if j["onlineMeeting"].is_null() {
                j["onlineMeeting"] = json!({ "joinUrl": join_url() });
            }
            j["onlineMeetingProvider"] = json!("teamsForBusiness");
        } else {
            j["onlineMeeting"] = Value::Null;
            j["onlineMeetingProvider"] = json!("unknown");
        }
    }
    j["changeKey"] = json!(new_id("DwAAABYA"));
    j["lastModifiedDateTime"] = json!(iso(Utc::now()));
}

/// Parses start/end into stored form. Returns (start, end, all_day, tz label).
fn parse_times(body: &Value, cur: Option<(&NaiveDateTime, &NaiveDateTime, bool, &str)>) -> Result<Option<(NaiveDateTime, NaiveDateTime, bool, String)>, GErr> {
    let has_times = body.get("start").is_some() || body.get("end").is_some() || body.get("isAllDay").is_some();
    if !has_times {
        return Ok(None);
    }
    let all_day = body["isAllDay"].as_bool().or(cur.map(|c| c.2)).unwrap_or(false);
    let convert = |v: &Value, field: &str| -> Result<(NaiveDateTime, String), GErr> {
        let (local, tz_name, tz) = parse_dt_tz(v, field)?;
        if all_day {
            if local.time() != chrono::NaiveTime::MIN {
                return Err(GErr::bad("Start and end times of an all-day event must be at midnight."));
            }
            Ok((local, tz_name))
        } else {
            Ok((to_utc(local, tz), tz_name))
        }
    };
    let (start, tz) = match body.get("start") {
        Some(v) => convert(v, "start")?,
        None => {
            let c = cur.ok_or_else(|| GErr::bad("start is required."))?;
            (*c.0, c.3.to_string())
        }
    };
    let end = match body.get("end") {
        Some(v) => convert(v, "end")?.0,
        None => {
            let c = cur.ok_or_else(|| GErr::bad("end is required."))?;
            start + (*c.1 - *c.0)
        }
    };
    if end < start {
        return Err(GErr::bad("The end time must be after the start time."));
    }
    if all_day && end == start {
        return Err(GErr::bad("An all-day event must span at least one day."));
    }
    Ok(Some((start, end, all_day, tz)))
}

pub fn new_event(mb: &Mailbox, cal_id: &str, body: &Value) -> Result<Ev, GErr> {
    let (start, end, all_day, tz) = parse_times(body, None)?.ok_or_else(|| GErr::bad("start and end are required."))?;
    let id = new_id("AAMkAE");
    let now = Utc::now();
    let mut j = json!({
        "createdDateTime": iso(now),
        "lastModifiedDateTime": iso(now),
        "changeKey": new_id("DwAAABYA"),
        "categories": [],
        "transactionId": body["transactionId"].clone(),
        "originalStartTimeZone": tz,
        "originalEndTimeZone": tz,
        "iCalUId": format!("040000008200E00074C5B7101A82E008{}", uuid::Uuid::new_v4().simple().to_string().to_uppercase()),
        "reminderMinutesBeforeStart": 15,
        "isReminderOn": true,
        "hasAttachments": false,
        "subject": "",
        "bodyPreview": "",
        "importance": "normal",
        "sensitivity": "normal",
        "isCancelled": false,
        "isOrganizer": true,
        "responseRequested": true,
        "showAs": if all_day { "free" } else { "busy" },
        "webLink": format!("https://outlook.office365.com/owa/?itemid={}&exvsurl=1&path=/calendar/item", id),
        "onlineMeetingUrl": null,
        "isOnlineMeeting": false,
        "onlineMeetingProvider": "unknown",
        "allowNewTimeProposals": true,
        "isDraft": false,
        "hideAttendees": false,
        "responseStatus": { "response": "organizer", "time": "0001-01-01T00:00:00Z" },
        "body": { "contentType": "html", "content": "" },
        "location": { "displayName": "", "locationType": "default", "uniqueIdType": "unknown" },
        "locations": [],
        "attendees": [],
        "organizer": mb.me(),
        "onlineMeeting": null,
    });
    apply_props(&mut j, body);
    if let Some(org) = body.get("organizer").filter(|o| !o.is_null()) {
        j["organizer"] = org.clone();
    }
    let recurrence = match body.get("recurrence") {
        Some(r) if !r.is_null() => Some(normalize_recurrence(r, &tz)?),
        _ => None,
    };
    Ok(Ev {
        id,
        cal_id: cal_id.to_string(),
        json: j,
        start,
        end,
        all_day,
        tz,
        recurrence,
        exceptions: HashMap::new(),
        deleted_occ: HashSet::new(),
    })
}

fn events_collection(mb: &mut Mailbox, cal_id: &str, req: &Req) -> GResult {
    match req.method {
        Method::GET => {
            let mut evs: Vec<&Ev> = mb.events.values().filter(|e| e.cal_id == cal_id).collect();
            evs.sort_by_key(|e| e.start);
            let tz = out_tz(req);
            let vals = evs.into_iter().map(|e| apply_select(render_event(e, None, &tz), req.q("select"))).collect();
            ok(paged(req, vals, 10))
        }
        Method::POST => {
            ensure_editable(mb, cal_id)?;
            let ev = new_event(mb, cal_id, &req.json()?)?;
            // Like Graph: the POST response is in the event's own zone, the
            // `Prefer: outlook.timezone` header only applies to GET.
            let v = render_event(&ev, None, &ev.tz);
            mb.events.insert(ev.id.clone(), ev);
            created(v)
        }
        _ => Err(GErr::bad("Unsupported method")),
    }
}

fn split_occ(id: &str) -> Option<(&str, &str)> {
    id.split_once(OCC_SEP)
}

fn event_item(mb: &mut Mailbox, id: &str, req: &Req) -> GResult {
    let tz = out_tz(req);
    if let Some((master, key)) = split_occ(id) {
        let ev = mb.events.get(master).ok_or_else(|| GErr::not_found("event"))?;
        let occ = occurrence_at(ev, key).ok_or_else(|| GErr::not_found("event occurrence"))?;
        let (o_start, o_end) = (occ.start, occ.end);
        match req.method {
            Method::GET => return ok(apply_select(render_event(ev, Some(occ), &tz), req.q("select"))),
            Method::PATCH => {
                ensure_editable(mb, &ev.cal_id.clone())?;
                let body = req.json()?;
                if body.get("recurrence").is_some_and(|r| !r.is_null()) {
                    return Err(GErr::bad("Recurrence can only be set on the series master."));
                }
                let ev = mb.events.get_mut(master).expect("checked");
                let all_day = ev.all_day;
                let tz_label = ev.tz.clone();
                let times = parse_times(&body, Some((&o_start, &o_end, all_day, &tz_label)))?;
                let entry = ev.exceptions.entry(key.to_string()).or_insert_with(|| Occ { start: o_start, end: o_end, patch: json!({}) });
                if let Some((s, e, _, _)) = times {
                    entry.start = s;
                    entry.end = e;
                }
                let mut base = ev.json.clone();
                crate::util::merge(&mut base, &entry.patch);
                apply_props(&mut base, &body);
                entry.patch = base;
                let ev = &mb.events[master];
                let occ = occurrence_at(ev, key).expect("exists");
                return ok(render_event(ev, Some(occ), &ev.tz));
            }
            Method::DELETE => {
                ensure_editable(mb, &ev.cal_id.clone())?;
                let ev = mb.events.get_mut(master).expect("checked");
                ev.exceptions.remove(key);
                ev.deleted_occ.insert(key.to_string());
                return no_content();
            }
            _ => return Err(GErr::bad("Unsupported method")),
        }
    }
    let ev = mb.events.get(id).ok_or_else(|| GErr::not_found("event"))?;
    match req.method {
        Method::GET => ok(apply_select(render_event(ev, None, &tz), req.q("select"))),
        Method::PATCH => {
            ensure_editable(mb, &ev.cal_id.clone())?;
            let body = req.json()?;
            let ev = mb.events.get_mut(id).expect("checked");
            let times = parse_times(&body, Some((&ev.start, &ev.end, ev.all_day, &ev.tz.clone())))?;
            if let Some((s, e, all_day, tzl)) = times {
                ev.start = s;
                ev.end = e;
                ev.all_day = all_day;
                ev.json["originalStartTimeZone"] = json!(tzl);
                ev.json["originalEndTimeZone"] = json!(tzl);
                ev.tz = tzl;
            }
            if let Some(r) = body.get("recurrence") {
                ev.recurrence = if r.is_null() { None } else { Some(normalize_recurrence(r, &ev.tz)?) };
                ev.exceptions.clear();
                ev.deleted_occ.clear();
            }
            apply_props(&mut ev.json, &body);
            // PATCH responses are in the event's own zone as well.
            ok(render_event(ev, None, &ev.tz))
        }
        Method::DELETE => {
            ensure_editable(mb, &ev.cal_id.clone())?;
            mb.events.remove(id);
            no_content()
        }
        _ => Err(GErr::bad("Unsupported method")),
    }
}

fn event_action(mb: &mut Mailbox, id: &str, action: &str, req: &Req) -> GResult {
    let response = match action {
        "accept" => "accepted",
        "tentativelyAccept" => "tentativelyAccepted",
        "decline" => "declined",
        "cancel" => "cancel",
        "dismissReminder" | "snoozeReminder" => return crate::util::accepted(),
        _ => return Err(GErr::new(StatusCode::BAD_REQUEST, "BadRequest", format!("Unsupported action '{action}'"))),
    };
    let _ = req.json()?;
    let (master, key) = match split_occ(id) {
        Some((m, k)) => (m, Some(k)),
        None => (id, None),
    };
    let ev = mb.events.get(master).ok_or_else(|| GErr::not_found("event"))?;
    if let Some(k) = key {
        occurrence_at(ev, k).ok_or_else(|| GErr::not_found("event occurrence"))?;
    }
    let is_org = ev.json["isOrganizer"].as_bool().unwrap_or(true);
    if response == "cancel" {
        if !is_org {
            return Err(GErr::new(StatusCode::BAD_REQUEST, "ErrorInvalidRequest", "Only the meeting organizer can cancel the meeting."));
        }
        let ev = mb.events.get_mut(master).expect("checked");
        match key {
            Some(k) => {
                ev.deleted_occ.insert(k.to_string());
            }
            None => {
                mb.events.remove(master);
            }
        }
        return crate::util::accepted();
    }
    if is_org {
        return Err(GErr::new(
            StatusCode::BAD_REQUEST,
            "ErrorInvalidRequest",
            "Your request can't be completed. You can't respond to this meeting because you're the meeting organizer.",
        ));
    }
    let status = json!({ "response": response, "time": iso(Utc::now()) });
    let show_as = if response == "tentativelyAccepted" { "tentative" } else { "busy" };
    let ev = mb.events.get_mut(master).expect("checked");
    match key {
        Some(k) if response == "declined" => {
            ev.exceptions.remove(k);
            ev.deleted_occ.insert(k.to_string());
        }
        Some(k) => {
            let occ = occurrence_at(ev, k).map(|o| (o.start, o.end)).expect("checked");
            let entry = ev.exceptions.entry(k.to_string()).or_insert_with(|| Occ { start: occ.0, end: occ.1, patch: json!({}) });
            entry.patch["responseStatus"] = status;
            entry.patch["showAs"] = json!(show_as);
        }
        None if response == "declined" => {
            // Like Exchange: a declined meeting disappears from the calendar.
            mb.events.remove(master);
        }
        None => {
            ev.json["responseStatus"] = status;
            ev.json["showAs"] = json!(show_as);
            ev.json["changeKey"] = json!(new_id("DwAAABYA"));
        }
    }
    crate::util::accepted()
}
