//! Import of iCalendar files (`.ics`): the VEVENTs of a file as events the UI
//! can show in the event window before they are saved to a calendar.

use chrono::{NaiveDate, NaiveDateTime, NaiveTime, SecondsFormat, TimeZone, Utc};
use serde::Serialize;

use crate::convert::{local_to_utc, resolve_tz};
use crate::model::{Address, Recurrence};

/// One VEVENT of an `.ics` file, shaped like an event of the app.
#[derive(Debug, Clone, Serialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct IcsEvent {
    pub uid: String,
    pub subject: String,
    /// UTC (`…Z`), or for an all-day event midnight UTC of the day. A
    /// floating time (no zone in the file) comes without `Z` and is read as
    /// local time by the page.
    pub start: String,
    pub end: String,
    pub is_all_day: bool,
    pub location: String,
    /// DESCRIPTION as HTML (escaped, line breaks kept).
    pub body_html: String,
    pub show_as: String,
    pub categories: Vec<String>,
    pub is_reminder_on: bool,
    pub reminder_minutes: i64,
    pub sensitivity: String,
    pub importance: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recurrence: Option<Recurrence>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub organizer: Option<Address>,
    pub attendees: Vec<Address>,
}

/// Content line: name, parameters (names upper case) and raw value.
struct Line {
    name: String,
    params: Vec<(String, String)>,
    value: String,
}

impl Line {
    fn param(&self, k: &str) -> Option<&str> {
        self.params.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
    }
}

/// Unfolds continuation lines (RFC 5545 3.1) and splits name;params:value.
fn content_lines(ics: &str) -> Vec<Line> {
    let ics = ics.trim_start_matches('\u{feff}');
    let mut raw: Vec<String> = Vec::new();
    for l in ics.split('\n') {
        let l = l.trim_end_matches('\r');
        if (l.starts_with(' ') || l.starts_with('\t')) && !raw.is_empty() {
            raw.last_mut().expect("non-empty").push_str(&l[1..]);
        } else {
            raw.push(l.to_string());
        }
    }
    raw.into_iter()
        .filter_map(|l| {
            // The value may contain ':' (URLs), a quoted parameter too.
            let mut in_quote = false;
            let split = l.char_indices().find(|&(_, c)| {
                if c == '"' {
                    in_quote = !in_quote;
                }
                c == ':' && !in_quote
            })?;
            let (head, value) = (&l[..split.0], &l[split.0 + 1..]);
            let mut parts = head.split(';');
            let name = parts.next().unwrap_or("").trim().to_ascii_uppercase();
            let params = parts
                .filter_map(|p| p.split_once('=').map(|(k, v)| (k.trim().to_ascii_uppercase(), v.trim_matches('"').to_string())))
                .collect();
            Some(Line { name, params, value: value.to_string() })
        })
        .collect()
}

fn unescape(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    let mut it = v.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('n') | Some('N') => out.push('\n'),
                Some(o) => out.push(o),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Splits a TEXT list at unescaped commas.
fn text_list(v: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut esc = false;
    for c in v.chars() {
        if esc {
            cur.push('\\');
            cur.push(c);
            esc = false;
        } else if c == '\\' {
            esc = true;
        } else if c == ',' {
            out.push(unescape(&cur));
            cur.clear();
        } else {
            cur.push(c);
        }
    }
    out.push(unescape(&cur));
    out.into_iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Plain text → HTML paragraph with links kept clickable.
fn text_to_html(text: &str) -> String {
    let text = text.trim();
    if text.is_empty() {
        return String::new();
    }
    let lines: Vec<String> = text
        .lines()
        .map(|l| {
            l.split(' ')
                .map(|w| {
                    if (w.starts_with("https://") || w.starts_with("http://")) && w.len() > 8 {
                        let e = html_escape(w);
                        format!("<a href=\"{e}\">{e}</a>")
                    } else {
                        html_escape(w)
                    }
                })
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect();
    format!("<p>{}</p>", lines.join("<br>"))
}

enum When {
    Utc(chrono::DateTime<Utc>),
    Floating(NaiveDateTime),
    Date(NaiveDate),
}

impl When {
    fn iso(&self) -> String {
        match self {
            When::Utc(t) => t.to_rfc3339_opts(SecondsFormat::Secs, true),
            When::Floating(n) => n.format("%Y-%m-%dT%H:%M:%S").to_string(),
            When::Date(d) => Utc.from_utc_datetime(&d.and_time(NaiveTime::MIN)).to_rfc3339_opts(SecondsFormat::Secs, true),
        }
    }
    fn naive(&self) -> NaiveDateTime {
        match self {
            When::Utc(t) => t.naive_utc(),
            When::Floating(n) => *n,
            When::Date(d) => d.and_time(NaiveTime::MIN),
        }
    }
}

fn parse_when(line: &Line) -> Option<When> {
    let v = line.value.trim();
    if line.param("VALUE").is_some_and(|x| x.eq_ignore_ascii_case("DATE")) || v.len() == 8 {
        return NaiveDate::parse_from_str(v, "%Y%m%d").ok().map(When::Date);
    }
    let utc = v.ends_with('Z') || v.ends_with('z');
    let n = NaiveDateTime::parse_from_str(v.trim_end_matches(['Z', 'z']), "%Y%m%dT%H%M%S")
        .or_else(|_| NaiveDateTime::parse_from_str(v.trim_end_matches(['Z', 'z']), "%Y%m%dT%H%M"))
        .ok()?;
    if utc {
        return Some(When::Utc(Utc.from_utc_datetime(&n)));
    }
    match line.param("TZID") {
        // Unknown custom zones (VTIMEZONE only in the file): wall time as local.
        Some(tz) => match resolve_tz(tz.trim_start_matches('/')) {
            Some(tz) => Some(When::Utc(local_to_utc(n, tz))),
            None => Some(When::Floating(n)),
        },
        None => Some(When::Floating(n)),
    }
}

/// ISO 8601 duration (`P1D`, `PT1H30M`, `-PT15M`) in minutes.
fn duration_minutes(v: &str) -> Option<i64> {
    let v = v.trim();
    let (neg, v) = match v.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, v.strip_prefix('+').unwrap_or(v)),
    };
    let v = v.strip_prefix('P')?;
    let mut total = 0i64;
    let mut num = String::new();
    let mut in_time = false;
    for c in v.chars() {
        match c {
            'T' => in_time = true,
            '0'..='9' => num.push(c),
            _ => {
                let n: i64 = num.parse().ok()?;
                num.clear();
                total += match (c, in_time) {
                    ('W', _) => n * 7 * 1440,
                    ('D', _) => n * 1440,
                    ('H', true) => n * 60,
                    ('M', true) => n,
                    ('S', true) => n / 60,
                    _ => return None,
                };
            }
        }
    }
    Some(if neg { -total } else { total })
}

const DAYS: [(&str, &str); 7] = [
    ("SU", "sunday"),
    ("MO", "monday"),
    ("TU", "tuesday"),
    ("WE", "wednesday"),
    ("TH", "thursday"),
    ("FR", "friday"),
    ("SA", "saturday"),
];

/// RRULE → the app's recurrence. Rules the app cannot show (e.g. hourly,
/// several month days) give `None`: the event is imported once.
fn recurrence(rule: &str, start: &When) -> Option<Recurrence> {
    let mut parts = std::collections::HashMap::new();
    for p in rule.split(';') {
        if let Some((k, v)) = p.split_once('=') {
            parts.insert(k.trim().to_ascii_uppercase(), v.trim().to_ascii_uppercase());
        }
    }
    let start_day = start.naive().date();
    let interval = parts.get("INTERVAL").and_then(|v| v.parse().ok()).unwrap_or(1i64).max(1);
    // BYDAY: [+-n]XX
    let mut days = Vec::new();
    let mut nth: Option<i64> = None;
    if let Some(by) = parts.get("BYDAY") {
        for d in by.split(',') {
            let d = d.trim();
            if d.len() < 2 {
                continue;
            }
            let (num, code) = d.split_at(d.len() - 2);
            let name = DAYS.iter().find(|(c, _)| *c == code).map(|(_, n)| n.to_string())?;
            if !num.is_empty() {
                nth = Some(num.trim_start_matches('+').parse().ok()?);
            }
            days.push(name);
        }
    }
    if nth.is_none() {
        if let Some(pos) = parts.get("BYSETPOS") {
            nth = pos.parse().ok();
        }
    }
    let index = |n: i64| match n {
        1 => Some("first"),
        2 => Some("second"),
        3 => Some("third"),
        4 => Some("fourth"),
        -1 => Some("last"),
        _ => None,
    };
    let month_day = match parts.get("BYMONTHDAY") {
        Some(v) => v.parse::<i64>().ok().filter(|d| (1..=31).contains(d))?,
        None => chrono::Datelike::day(&start_day) as i64,
    };
    let month = match parts.get("BYMONTH") {
        Some(v) => v.parse::<i64>().ok().filter(|m| (1..=12).contains(m))?,
        None => chrono::Datelike::month(&start_day) as i64,
    };
    let weekday = DAYS[chrono::Datelike::weekday(&start_day).num_days_from_sunday() as usize].1.to_string();
    let mut r = Recurrence {
        pattern: String::new(),
        interval,
        days_of_week: vec![],
        day_of_month: 0,
        month: 0,
        index: None,
        // The event window takes the start date of the form.
        start_date: String::new(),
        end_date: None,
        occurrences: None,
    };
    match parts.get("FREQ").map(String::as_str)? {
        "DAILY" => r.pattern = "daily".into(),
        "WEEKLY" => {
            r.pattern = "weekly".into();
            r.days_of_week = if days.is_empty() { vec![weekday] } else { days };
        }
        "MONTHLY" if !days.is_empty() => {
            r.pattern = "relativeMonthly".into();
            r.index = Some(index(nth.unwrap_or(1))?.into());
            r.days_of_week = days;
        }
        "MONTHLY" => {
            r.pattern = "absoluteMonthly".into();
            r.day_of_month = month_day;
        }
        "YEARLY" if !days.is_empty() => {
            r.pattern = "relativeYearly".into();
            r.index = Some(index(nth.unwrap_or(1))?.into());
            r.days_of_week = days;
            r.month = month;
        }
        "YEARLY" => {
            r.pattern = "absoluteYearly".into();
            r.day_of_month = month_day;
            r.month = month;
        }
        _ => return None,
    }
    if let Some(n) = parts.get("COUNT").and_then(|v| v.parse::<i64>().ok()).filter(|n| *n > 0) {
        r.occurrences = Some(n);
    } else if let Some(u) = parts.get("UNTIL") {
        let fake = Line { name: String::new(), params: vec![], value: u.clone() };
        let until = parse_when(&fake)?;
        r.end_date = Some(until.naive().date().format("%Y-%m-%d").to_string());
    }
    Some(r)
}

fn address(line: &Line) -> Option<Address> {
    let v = line.value.trim();
    let address = v
        .strip_prefix("mailto:")
        .or_else(|| v.strip_prefix("MAILTO:"))
        .unwrap_or(v)
        .trim()
        .to_string();
    if address.is_empty() || !address.contains('@') {
        return None;
    }
    Some(Address { name: line.param("CN").map(unescape).unwrap_or_default(), address })
}

/// The events of an iCalendar file. Exceptions of a series (`RECURRENCE-ID`)
/// and cancelled events are left out.
pub fn parse_ics(ics: &str) -> Vec<IcsEvent> {
    let lines = content_lines(ics);
    let mut out = Vec::new();
    let mut depth: Vec<String> = Vec::new();
    let mut cur: Vec<&Line> = Vec::new();
    for l in &lines {
        match l.name.as_str() {
            "BEGIN" => {
                let comp = l.value.trim().to_ascii_uppercase();
                if comp == "VEVENT" && depth.last().map(String::as_str) != Some("VEVENT") {
                    cur.clear();
                }
                depth.push(comp);
            }
            "END" => {
                let comp = l.value.trim().to_ascii_uppercase();
                if depth.last() == Some(&comp) {
                    depth.pop();
                }
                if comp == "VEVENT" && depth.iter().all(|d| d != "VEVENT") {
                    if let Some(ev) = event(&cur) {
                        out.push(ev);
                    }
                    cur.clear();
                }
            }
            // Only the event's own properties, not those of a VALARM.
            _ if depth.last().map(String::as_str) == Some("VEVENT") => cur.push(l),
            // A VALARM's TRIGGER becomes the event's reminder.
            "TRIGGER" if depth.last().map(String::as_str) == Some("VALARM") && depth.iter().any(|d| d == "VEVENT") => cur.push(l),
            _ => {}
        }
    }
    out
}

fn event(props: &[&Line]) -> Option<IcsEvent> {
    let get = |n: &str| props.iter().find(|l| l.name == n).copied();
    if get("RECURRENCE-ID").is_some() {
        return None;
    }
    if get("STATUS").is_some_and(|l| l.value.trim().eq_ignore_ascii_case("CANCELLED")) {
        return None;
    }
    let start = parse_when(get("DTSTART")?)?;
    let is_all_day = matches!(start, When::Date(_));
    let end = match get("DTEND").and_then(parse_when) {
        Some(e) if e.naive() > start.naive() => e,
        _ => {
            let mins = get("DURATION").and_then(|l| duration_minutes(&l.value)).filter(|m| *m > 0);
            match (&start, mins) {
                (When::Date(d), m) => When::Date(*d + chrono::Duration::days((m.unwrap_or(1440) / 1440).max(1))),
                (When::Utc(t), m) => When::Utc(*t + chrono::Duration::minutes(m.unwrap_or(30))),
                (When::Floating(n), m) => When::Floating(*n + chrono::Duration::minutes(m.unwrap_or(30))),
            }
        }
    };
    let text = |n: &str| get(n).map(|l| unescape(&l.value).trim().to_string()).unwrap_or_default();
    let description = text("DESCRIPTION");
    let location = text("LOCATION").replace('\n', ", ");
    let mut body_html = props
        .iter()
        .find(|l| l.name == "X-ALT-DESC" && l.param("FMTTYPE").is_some_and(|f| f.eq_ignore_ascii_case("text/html")))
        .map(|l| unescape(&l.value))
        .filter(|h| !h.trim().is_empty())
        .unwrap_or_else(|| text_to_html(&description));
    let organizer = get("ORGANIZER").and_then(address);
    let attendees: Vec<Address> = props.iter().filter(|l| l.name == "ATTENDEE").filter_map(|l| address(l)).collect();
    if let Some(o) = &organizer {
        let who = if o.name.is_empty() { o.address.clone() } else { format!("{} <{}>", o.name, o.address) };
        body_html.push_str(&format!("<p>Organisator: {}</p>", html_escape(&who)));
    }
    let show_as = match get("TRANSP").map(|l| l.value.trim().to_ascii_uppercase()) {
        Some(t) if t == "TRANSPARENT" => "free",
        _ => match get("X-MICROSOFT-CDO-BUSYSTATUS").map(|l| l.value.trim().to_ascii_uppercase()).as_deref() {
            Some("FREE") => "free",
            Some("TENTATIVE") => "tentative",
            Some("OOF") => "oof",
            Some("WORKINGELSEWHERE") => "workingElsewhere",
            _ if is_all_day => "free",
            _ => "busy",
        },
    };
    let reminder = props
        .iter()
        .filter(|l| l.name == "TRIGGER" && !l.param("VALUE").is_some_and(|v| v.eq_ignore_ascii_case("DATE-TIME")))
        .filter_map(|l| duration_minutes(&l.value))
        .find(|m| *m <= 0)
        .map(|m| -m);
    let sensitivity = match get("CLASS").map(|l| l.value.trim().to_ascii_uppercase()).as_deref() {
        Some("PRIVATE") => "private",
        Some("CONFIDENTIAL") => "confidential",
        _ => "normal",
    };
    let importance = match get("PRIORITY").and_then(|l| l.value.trim().parse::<i64>().ok()) {
        Some(p) if (1..=4).contains(&p) => "high",
        Some(p) if p >= 6 => "low",
        _ => "normal",
    };
    let categories = props.iter().filter(|l| l.name == "CATEGORIES").flat_map(|l| text_list(&l.value)).collect();
    Some(IcsEvent {
        uid: text("UID"),
        subject: text("SUMMARY").replace('\n', " "),
        start: start.iso(),
        end: end.iso(),
        is_all_day,
        location,
        body_html,
        show_as: show_as.into(),
        categories,
        is_reminder_on: reminder.is_some(),
        reminder_minutes: reminder.unwrap_or(15),
        sensitivity: sensitivity.into(),
        importance: importance.into(),
        recurrence: get("RRULE").and_then(|l| recurrence(&l.value, &start)),
        organizer,
        attendees,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_event_with_zone_alarm_and_description() {
        let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:1@x\r\nSUMMARY:Zahnarzt\\, Kontrolle\r\n\
DTSTART;TZID=Europe/Berlin:20261203T093000\r\nDTEND;TZID=Europe/Berlin:20261203T101500\r\n\
LOCATION:Praxis Dr. Müller\\, Hauptstr. 1\r\nDESCRIPTION:Bitte Karte mitbringen.\\nInfos: https://e\r\n xample.com/a\r\n\
ORGANIZER;CN=\"Praxis\":mailto:info@praxis.de\r\nBEGIN:VALARM\r\nTRIGGER:-PT1H\r\nACTION:DISPLAY\r\nEND:VALARM\r\n\
END:VEVENT\r\nEND:VCALENDAR\r\n";
        let evs = parse_ics(ics);
        assert_eq!(evs.len(), 1);
        let e = &evs[0];
        assert_eq!(e.subject, "Zahnarzt, Kontrolle");
        assert_eq!(e.start, "2026-12-03T08:30:00Z");
        assert_eq!(e.end, "2026-12-03T09:15:00Z");
        assert_eq!(e.location, "Praxis Dr. Müller, Hauptstr. 1");
        assert!(e.body_html.contains("Bitte Karte mitbringen.<br>Infos: <a href=\"https://example.com/a\">"), "{}", e.body_html);
        assert!(e.body_html.contains("Organisator: Praxis &lt;info@praxis.de&gt;"), "{}", e.body_html);
        assert!(e.is_reminder_on);
        assert_eq!(e.reminder_minutes, 60);
        assert_eq!(e.show_as, "busy");
        assert!(!e.is_all_day);
    }

    #[test]
    fn all_day_floating_duration_and_windows_zone() {
        let ics = "BEGIN:VCALENDAR\nBEGIN:VEVENT\nSUMMARY:Urlaub\nDTSTART;VALUE=DATE:20261224\nDTEND;VALUE=DATE:20261227\nEND:VEVENT\n\
BEGIN:VEVENT\nSUMMARY:Lokal\nDTSTART:20261201T140000\nDURATION:PT1H30M\nEND:VEVENT\n\
BEGIN:VEVENT\nSUMMARY:Outlook\nDTSTART;TZID=W. Europe Standard Time:20260701T100000\nDTEND;TZID=W. Europe Standard Time:20260701T110000\nEND:VEVENT\n\
BEGIN:VEVENT\nSUMMARY:Abgesagt\nSTATUS:CANCELLED\nDTSTART:20261201T140000Z\nEND:VEVENT\nEND:VCALENDAR\n";
        let evs = parse_ics(ics);
        assert_eq!(evs.len(), 3);
        assert!(evs[0].is_all_day);
        assert_eq!(evs[0].start, "2026-12-24T00:00:00Z");
        assert_eq!(evs[0].end, "2026-12-27T00:00:00Z");
        assert_eq!(evs[0].show_as, "free");
        assert_eq!(evs[1].start, "2026-12-01T14:00:00");
        assert_eq!(evs[1].end, "2026-12-01T15:30:00");
        assert_eq!(evs[2].start, "2026-07-01T08:00:00Z");
    }

    #[test]
    fn recurrence_rules() {
        let w = When::Date(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()); // Monday
        let r = recurrence("FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE;COUNT=10", &w).unwrap();
        assert_eq!((r.pattern.as_str(), r.interval, r.occurrences), ("weekly", 2, Some(10)));
        assert_eq!(r.days_of_week, vec!["monday", "wednesday"]);
        let r = recurrence("FREQ=MONTHLY;BYDAY=-1FR;UNTIL=20271231T235959Z", &w).unwrap();
        assert_eq!((r.pattern.as_str(), r.index.as_deref()), ("relativeMonthly", Some("last")));
        assert_eq!(r.end_date.as_deref(), Some("2027-12-31"));
        let r = recurrence("FREQ=YEARLY", &w).unwrap();
        assert_eq!((r.pattern.as_str(), r.day_of_month, r.month), ("absoluteYearly", 2, 11));
        let r = recurrence("FREQ=WEEKLY", &w).unwrap();
        assert_eq!(r.days_of_week, vec!["monday"]);
        assert!(recurrence("FREQ=HOURLY", &w).is_none());
    }

    #[test]
    fn links_categories_and_exceptions_skipped() {
        let ics = "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:a\nSUMMARY:Jour fixe\nDTSTART:20261102T090000Z\nDTEND:20261102T093000Z\n\
RRULE:FREQ=WEEKLY;BYDAY=MO\nDESCRIPTION:Beitreten: https://teams.microsoft.com/l/meetup-join/19%3a abc\nCATEGORIES:Arbeit,Team\nEND:VEVENT\n\
BEGIN:VEVENT\nUID:a\nRECURRENCE-ID:20261109T090000Z\nSUMMARY:Jour fixe (verschoben)\nDTSTART:20261109T100000Z\nDTEND:20261109T103000Z\nEND:VEVENT\nEND:VCALENDAR\n";
        let evs = parse_ics(ics);
        assert_eq!(evs.len(), 1);
        assert!(evs[0].body_html.contains("href=\"https://teams.microsoft.com/l/meetup-join/19%3a\""));
        assert_eq!(evs[0].categories, vec!["Arbeit", "Team"]);
        assert_eq!(evs[0].recurrence.as_ref().unwrap().pattern, "weekly");
    }

    #[test]
    fn garbage_gives_nothing() {
        assert!(parse_ics("").is_empty());
        assert!(parse_ics("hello world").is_empty());
        assert!(parse_ics("BEGIN:VEVENT\nSUMMARY:x\nEND:VEVENT").is_empty());
    }
}
