//! Calendar edge cases against the fake Graph server: time zones (incl. the
//! POST/PATCH-in-event-zone behaviour of real Graph), DST transitions,
//! all-day and overnight events, recurring series, validation.


use chrono::{TimeZone, Utc};
use crate::common::*;
use mailcore::model::*;
use serde_json::{json, Value};

fn now() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 20, 10, 0, 0).unwrap()
}

#[tokio::test]
async fn create_event_in_berlin_returns_utc_instant() {
    // Regression (live M365): 10:00 Europe/Berlin on 2026-11-19 came back as 10:00Z.
    let ctx = setup_at(now()).await;
    let cal = ctx.default_calendar().await;
    let ev = ctx.create_event(&cal, ev_input("Live-Test", "2026-11-19T10:00:00", "2026-11-19T11:00:00")).await;
    assert_eq!(ev.start, "2026-11-19T09:00:00Z");
    assert_eq!(ev.end, "2026-11-19T10:00:00Z");

    // The same instant everywhere: detail and calendar view.
    let d: EventDetail = call(&ctx.svc, "get_event", json!({"accountId": ctx.account, "eventId": ev.id})).await;
    assert_eq!(d.event.start, "2026-11-19T09:00:00Z");
    let list = ctx.events(&cal, "2026-11-19T00:00:00Z", "2026-11-20T00:00:00Z").await;
    let listed = list.iter().find(|e| e.id == ev.id).expect("listed");
    assert_eq!((listed.start.as_str(), listed.end.as_str()), ("2026-11-19T09:00:00Z", "2026-11-19T10:00:00Z"));

    // Summer time: UTC+2.
    let ev = ctx.create_event(&cal, ev_input("Sommer", "2026-07-01T10:00:00", "2026-07-01T10:30:00")).await;
    assert_eq!(ev.start, "2026-07-01T08:00:00Z");
}

#[tokio::test]
async fn update_event_returns_utc_instant() {
    let ctx = setup_at(now()).await;
    let cal = ctx.default_calendar().await;
    let ev = ctx.create_event(&cal, ev_input("Termin", "2026-11-19T10:00:00", "2026-11-19T11:00:00")).await;
    let up: Event = call(
        &ctx.svc,
        "update_event",
        json!({"accountId": ctx.account, "eventId": ev.id, "input": ev_input("Termin", "2027-06-10T15:15:00", "2027-06-10T16:00:00")}),
    )
    .await;
    assert_eq!(up.start, "2027-06-10T13:15:00Z");
    assert_eq!(up.end, "2027-06-10T14:00:00Z");
    // Server state agrees.
    let stored = ctx.fake.state.events(ME).into_iter().find(|e| e["id"] == json!(ev.id)).unwrap();
    assert_eq!(stored["start"]["dateTime"], "2027-06-10T13:15:00.0000000");
}

#[tokio::test]
async fn windows_and_foreign_time_zones() {
    let ctx = setup_at(now()).await;
    let cal = ctx.default_calendar().await;
    let mut input = ev_input("Windows-Zone", "2026-11-19T10:00:00", "2026-11-19T11:00:00");
    input["timeZone"] = json!("W. Europe Standard Time");
    let ev = ctx.create_event(&cal, input).await;
    assert_eq!(ev.start, "2026-11-19T09:00:00Z");

    let mut input = ev_input("New York", "2026-11-19T10:00:00", "2026-11-19T11:00:00");
    input["timeZone"] = json!("America/New_York");
    let ev = ctx.create_event(&cal, input).await;
    assert_eq!(ev.start, "2026-11-19T15:00:00Z");

    let mut input = ev_input("UTC", "2026-11-19T10:00:00", "2026-11-19T11:00:00");
    input["timeZone"] = json!("UTC");
    let ev = ctx.create_event(&cal, input).await;
    assert_eq!(ev.start, "2026-11-19T10:00:00Z");
}

#[tokio::test]
async fn unresolvable_zone_in_response_is_reread_in_utc() {
    let ctx = setup_at(now()).await;
    let cal = ctx.default_calendar().await;
    let mut input = ev_input("Serie aus ICS", "2026-11-02T10:00:00", "2026-11-02T11:00:00");
    input["recurrence"] = json!({"pattern": "weekly", "interval": 1, "daysOfWeek": ["monday"], "startDate": "2026-11-02", "occurrences": 3});
    let series = ctx.create_event(&cal, input).await;
    // Exchange labels series from foreign invitations "Customized Time Zone".
    assert!(ctx.fake.state.set_event_time_zone(ME, &series.id, "Customized Time Zone"));
    let occ = ctx.events(&cal, "2026-11-09T00:00:00Z", "2026-11-10T00:00:00Z").await;
    let occ = occ.iter().find(|e| e.series_master_id.as_deref() == Some(series.id.as_str())).expect("occurrence");
    assert_eq!(occ.start, "2026-11-09T09:00:00Z");
    let up: Event = call(
        &ctx.svc,
        "update_event",
        json!({"accountId": ctx.account, "eventId": occ.id, "input": ev_input("Verschoben", "2026-11-09T12:00:00", "2026-11-09T13:00:00")}),
    )
    .await;
    assert_eq!(up.subject, "Verschoben");
    assert_eq!(up.start, "2026-11-09T11:00:00Z");
    assert_eq!(up.kind, "exception");
}

#[tokio::test]
async fn dst_transitions() {
    let ctx = setup_at(now()).await;
    let cal = ctx.default_calendar().await;
    // Spans the spring-forward night (last Sunday of March): 3 wall hours, 2 real.
    let ev = ctx.create_event(&cal, ev_input("Nachtschicht", "2027-03-28T01:00:00", "2027-03-28T04:00:00")).await;
    assert_eq!((ev.start.as_str(), ev.end.as_str()), ("2027-03-28T00:00:00Z", "2027-03-28T02:00:00Z"));
    // Fall back (last Sunday of October): 02:30 exists twice, the first (CEST) wins.
    let ev = ctx.create_event(&cal, ev_input("Doppelt", "2026-10-25T02:30:00", "2026-10-25T03:30:00")).await;
    assert_eq!(ev.start, "2026-10-25T00:30:00Z");
    assert_eq!(ev.end, "2026-10-25T02:30:00Z");
    // A day after each switch.
    let ev = ctx.create_event(&cal, ev_input("Montag", "2026-10-26T09:00:00", "2026-10-26T10:00:00")).await;
    assert_eq!(ev.start, "2026-10-26T08:00:00Z");
    let ev = ctx.create_event(&cal, ev_input("Montag", "2027-03-29T09:00:00", "2027-03-29T10:00:00")).await;
    assert_eq!(ev.start, "2027-03-29T07:00:00Z");
}

#[tokio::test]
async fn daily_series_keeps_wall_time_across_dst() {
    let ctx = setup_at(now()).await;
    let cal = ctx.default_calendar().await;
    let mut input = ev_input("Daily", "2026-10-23T09:00:00", "2026-10-23T09:15:00");
    input["recurrence"] = json!({"pattern": "daily", "interval": 1, "startDate": "2026-10-23", "occurrences": 5});
    let s = ctx.create_event(&cal, input).await;
    assert_eq!(s.kind, "seriesMaster");
    let occ: Vec<String> = ctx
        .events(&cal, "2026-10-22T00:00:00Z", "2026-11-01T00:00:00Z")
        .await
        .into_iter()
        .filter(|e| e.series_master_id.as_deref() == Some(s.id.as_str()))
        .map(|e| e.start)
        .collect();
    assert_eq!(
        occ,
        [
            "2026-10-23T07:00:00Z",
            "2026-10-24T07:00:00Z",
            "2026-10-25T08:00:00Z",
            "2026-10-26T08:00:00Z",
            "2026-10-27T08:00:00Z"
        ]
    );

    let mut input = ev_input("Daily März", "2027-03-27T09:00:00", "2027-03-27T09:15:00");
    input["recurrence"] = json!({"pattern": "daily", "interval": 1, "startDate": "2027-03-27", "endDate": "2027-03-29"});
    let s = ctx.create_event(&cal, input).await;
    let occ: Vec<String> = ctx
        .events(&cal, "2027-03-26T00:00:00Z", "2027-04-01T00:00:00Z")
        .await
        .into_iter()
        .filter(|e| e.series_master_id.as_deref() == Some(s.id.as_str()))
        .map(|e| e.start)
        .collect();
    assert_eq!(occ, ["2027-03-27T08:00:00Z", "2027-03-28T07:00:00Z", "2027-03-29T07:00:00Z"]);
}

#[tokio::test]
async fn all_day_events_keep_their_dates() {
    let ctx = setup_at(now()).await;
    let cal = ctx.default_calendar().await;
    // Multi-day over the October DST switch; the UI sends end = last day + 1.
    let mut input = ev_input("Urlaub", "2026-10-24T00:00:00", "2026-10-27T00:00:00");
    input["isAllDay"] = json!(true);
    let ev = ctx.create_event(&cal, input).await;
    assert!(ev.is_all_day);
    assert_eq!((ev.start.as_str(), ev.end.as_str()), ("2026-10-24T00:00:00Z", "2026-10-27T00:00:00Z"));
    let d: EventDetail = call(&ctx.svc, "get_event", json!({"accountId": ctx.account, "eventId": ev.id})).await;
    assert_eq!((d.event.start.as_str(), d.event.end.as_str()), ("2026-10-24T00:00:00Z", "2026-10-27T00:00:00Z"));
    // Visible on a day in the middle, not on the day after.
    assert!(ctx.events(&cal, "2026-10-26T00:00:00Z", "2026-10-26T23:59:59Z").await.iter().any(|e| e.id == ev.id));
    assert!(!ctx.events(&cal, "2026-10-27T00:00:00Z", "2026-10-27T23:59:59Z").await.iter().any(|e| e.id == ev.id));

    // A time of day on an all-day event is normalised to midnight.
    let mut input = ev_input("Feiertag", "2026-12-24T10:00:00", "2026-12-25T10:00:00");
    input["isAllDay"] = json!(true);
    let ev = ctx.create_event(&cal, input).await;
    assert_eq!((ev.start.as_str(), ev.end.as_str()), ("2026-12-24T00:00:00Z", "2026-12-25T00:00:00Z"));

    // Zero days is rejected with a German message, before Graph is asked.
    let before = ctx.fake.state.request_count();
    let mut input = ev_input("Leer", "2026-12-24T00:00:00", "2026-12-24T00:00:00");
    input["isAllDay"] = json!(true);
    let err = call_err(&ctx.svc, "create_event", json!({"accountId": ctx.account, "calendarId": cal, "input": input})).await;
    assert!(err.contains("mindestens einen Tag"), "{err}");
    assert_eq!(ctx.fake.state.request_count(), before);
}

#[tokio::test]
async fn overnight_event_shows_on_both_days() {
    let ctx = setup_at(now()).await;
    let cal = ctx.default_calendar().await;
    let ev = ctx.create_event(&cal, ev_input("Release", "2026-11-19T23:00:00", "2026-11-20T01:30:00")).await;
    assert_eq!((ev.start.as_str(), ev.end.as_str()), ("2026-11-19T22:00:00Z", "2026-11-20T00:30:00Z"));
    // Local days in Berlin, expressed in UTC.
    assert!(ctx.events(&cal, "2026-11-18T23:00:00Z", "2026-11-19T23:00:00Z").await.iter().any(|e| e.id == ev.id));
    assert!(ctx.events(&cal, "2026-11-19T23:00:00Z", "2026-11-20T23:00:00Z").await.iter().any(|e| e.id == ev.id));
    assert!(!ctx.events(&cal, "2026-11-20T23:00:00Z", "2026-11-21T23:00:00Z").await.iter().any(|e| e.id == ev.id));
}

#[tokio::test]
async fn validation_errors_are_german_and_local() {
    let ctx = setup_at(now()).await;
    let cal = ctx.default_calendar().await;
    let before = ctx.fake.state.request_count();
    let create = |input: Value| {
        let svc = ctx.svc.clone();
        let args = json!({"accountId": ctx.account, "calendarId": cal, "input": input});
        async move { call_err(&svc, "create_event", args).await }
    };
    let err = create(ev_input("x", "2026-11-19T11:00:00", "2026-11-19T10:00:00")).await;
    assert!(err.contains("Das Ende liegt vor dem Beginn"), "{err}");
    let err = create(ev_input("x", "", "2026-11-19T10:00:00")).await;
    assert!(err.contains("Beginn und Ende"), "{err}");
    let err = create(ev_input("x", "morgen", "2026-11-19T10:00:00")).await;
    assert!(err.contains("Beginn und Ende"), "{err}");
    let mut input = ev_input("x", "2026-11-19T10:00:00", "2026-11-19T11:00:00");
    input["timeZone"] = json!("Mars/Olympus_Mons");
    let err = create(input).await;
    assert!(err.contains("Unbekannte Zeitzone"), "{err}");

    let with_rec = |rec: Value| {
        let mut i = ev_input("Serie", "2026-11-19T10:00:00", "2026-11-19T11:00:00");
        i["recurrence"] = rec;
        i
    };
    let err = create(with_rec(json!({"pattern": "weekly", "startDate": "2026-11-19"}))).await;
    assert!(err.contains("Wochentag"), "{err}");
    let err = create(with_rec(json!({"pattern": "hourly", "startDate": "2026-11-19"}))).await;
    assert!(err.contains("Serienmuster"), "{err}");
    let err = create(with_rec(json!({"pattern": "daily", "startDate": "2026-11-19", "endDate": "2026-11-01"}))).await;
    assert!(err.contains("vor ihrem Beginn"), "{err}");
    let err = create(with_rec(json!({"pattern": "daily", "startDate": "2026-11-19", "occurrences": 0}))).await;
    assert!(err.contains("Anzahl"), "{err}");
    let err = create(with_rec(json!({"pattern": "absoluteMonthly", "dayOfMonth": 32, "startDate": "2026-11-19"}))).await;
    assert!(err.contains("Tag im Monat"), "{err}");
    let err = create(with_rec(json!({"pattern": "daily", "startDate": ""}))).await;
    assert!(err.contains("Startdatum"), "{err}");
    let err = create(with_rec(json!({"pattern": "relativeMonthly", "daysOfWeek": ["tuesday"], "index": "fifth", "startDate": "2026-11-19"}))).await;
    assert!(err.contains("Woche im Monat"), "{err}");
    assert_eq!(ctx.fake.state.request_count(), before, "invalid input must not reach Graph");

    // Zero-length events and empty subjects are fine (Outlook allows both).
    let ev = ctx.create_event(&cal, ev_input("", "2026-11-19T10:00:00", "2026-11-19T10:00:00")).await;
    assert_eq!(ev.subject, "");
    assert_eq!(ev.start, ev.end);

    let err = call_err(
        &ctx.svc,
        "list_events",
        json!({"accountId": ctx.account, "calendarIds": [cal], "start": "2026-11-20T00:00:00Z", "end": "2026-11-19T00:00:00Z"}),
    )
    .await;
    assert!(err.contains("Ungültiger Zeitraum"), "{err}");
    let err = call_err(
        &ctx.svc,
        "list_events",
        json!({"accountId": ctx.account, "calendarIds": [cal], "start": "", "end": "2026-11-19T00:00:00Z"}),
    )
    .await;
    assert!(err.contains("Ungültiger Zeitraum"), "{err}");
}

async fn series_starts(ctx: &Ctx, cal: &str, master: &str, from: &str, to: &str) -> Vec<String> {
    ctx.events(cal, from, to)
        .await
        .into_iter()
        .filter(|e| e.series_master_id.as_deref() == Some(master))
        .map(|e| e.start)
        .collect()
}

#[tokio::test]
async fn monthly_and_yearly_patterns() {
    let ctx = setup_at(now()).await;
    let cal = ctx.default_calendar().await;
    // Day 31 falls on the last day of shorter months.
    let mut input = ev_input("Monatsende", "2026-10-31T09:00:00", "2026-10-31T10:00:00");
    input["recurrence"] = json!({"pattern": "absoluteMonthly", "interval": 1, "dayOfMonth": 31, "startDate": "2026-10-31", "occurrences": 5});
    let s = ctx.create_event(&cal, input).await;
    let r = s.recurrence.clone().unwrap();
    assert_eq!((r.pattern.as_str(), r.day_of_month, r.occurrences), ("absoluteMonthly", 31, Some(5)));
    let starts = series_starts(&ctx, &cal, &s.id, "2026-10-01T00:00:00Z", "2027-06-01T00:00:00Z").await;
    assert_eq!(
        starts,
        ["2026-10-31T08:00:00Z", "2026-11-30T08:00:00Z", "2026-12-31T08:00:00Z", "2027-01-31T08:00:00Z", "2027-02-28T08:00:00Z"]
    );

    // Second Tuesday of every month: index and weekday must survive.
    let mut input = ev_input("Jour fixe", "2026-11-10T14:00:00", "2026-11-10T15:00:00");
    input["recurrence"] = json!({"pattern": "relativeMonthly", "interval": 1, "daysOfWeek": ["tuesday"], "index": "second",
        "startDate": "2026-11-10", "endDate": "2027-01-31"});
    let s = ctx.create_event(&cal, input.clone()).await;
    let r = s.recurrence.clone().unwrap();
    assert_eq!(r.index.as_deref(), Some("second"));
    assert_eq!(r.days_of_week, ["tuesday"]);
    assert_eq!(r.end_date.as_deref(), Some("2027-01-31"));
    let want = ["2026-11-10T13:00:00Z", "2026-12-08T13:00:00Z", "2027-01-12T13:00:00Z"];
    assert_eq!(series_starts(&ctx, &cal, &s.id, "2026-11-01T00:00:00Z", "2027-03-01T00:00:00Z").await, want);

    // The editor sends the recurrence it received back on save.
    let mut edit = input.clone();
    edit["subject"] = json!("Jour fixe (neu)");
    edit["recurrence"] = serde_json::to_value(&r).unwrap();
    let up: Event = call(&ctx.svc, "update_event", json!({"accountId": ctx.account, "eventId": s.id, "input": edit})).await;
    assert_eq!(up.subject, "Jour fixe (neu)");
    assert_eq!(up.recurrence.as_ref().and_then(|r| r.index.as_deref()), Some("second"));
    assert_eq!(series_starts(&ctx, &cal, &s.id, "2026-11-01T00:00:00Z", "2027-03-01T00:00:00Z").await, want);

    // Last Friday of March, yearly, no end.
    let mut input = ev_input("Jahresabschluss", "2027-03-26T16:00:00", "2027-03-26T17:00:00");
    input["recurrence"] = json!({"pattern": "relativeYearly", "interval": 1, "daysOfWeek": ["friday"], "index": "last", "month": 3,
        "startDate": "2027-03-26"});
    let s = ctx.create_event(&cal, input).await;
    assert!(s.recurrence.as_ref().unwrap().end_date.is_none() && s.recurrence.as_ref().unwrap().occurrences.is_none());
    // 2027-03-26 is CET (before the switch), 2028-03-31 is CEST.
    let starts = series_starts(&ctx, &cal, &s.id, "2027-01-01T00:00:00Z", "2029-01-01T00:00:00Z").await;
    assert_eq!(starts, ["2027-03-26T15:00:00Z", "2028-03-31T14:00:00Z"]);

    // 29 February, yearly.
    let mut input = ev_input("Schalttag", "2028-02-29T12:00:00", "2028-02-29T13:00:00");
    input["recurrence"] = json!({"pattern": "absoluteYearly", "interval": 1, "dayOfMonth": 29, "month": 2, "startDate": "2028-02-29", "occurrences": 2});
    let s = ctx.create_event(&cal, input).await;
    let starts = series_starts(&ctx, &cal, &s.id, "2028-01-01T00:00:00Z", "2030-01-01T00:00:00Z").await;
    assert_eq!(starts, ["2028-02-29T11:00:00Z", "2029-02-28T11:00:00Z"]);
}

#[tokio::test]
async fn occurrence_and_series_changes() {
    let ctx = setup_at(now()).await;
    let cal = ctx.default_calendar().await;
    let mut input = ev_input("Weekly", "2026-11-02T10:00:00", "2026-11-02T10:30:00");
    let rec = json!({"pattern": "weekly", "interval": 1, "daysOfWeek": ["monday"], "startDate": "2026-11-02", "occurrences": 4});
    input["recurrence"] = rec.clone();
    let s = ctx.create_event(&cal, input.clone()).await;
    let (from, to) = ("2026-11-01T00:00:00Z", "2026-12-01T00:00:00Z");
    let occ: Vec<Event> = ctx
        .events(&cal, from, to)
        .await
        .into_iter()
        .filter(|e| e.series_master_id.as_deref() == Some(s.id.as_str()))
        .collect();
    assert_eq!(occ.len(), 4);
    assert!(occ.iter().all(|o| o.kind == "occurrence" && o.recurrence.is_none()));

    // Move one occurrence; the editor still carries the series' recurrence.
    let mut moved = ev_input("Weekly (verschoben)", "2026-11-10T11:00:00", "2026-11-10T11:30:00");
    moved["recurrence"] = rec.clone();
    let up: Event = call(&ctx.svc, "update_event", json!({"accountId": ctx.account, "eventId": occ[1].id, "input": moved})).await;
    assert_eq!(up.kind, "exception");
    assert_eq!(up.start, "2026-11-10T10:00:00Z");
    // Delete another occurrence.
    let _: Value = call(&ctx.svc, "delete_event", json!({"accountId": ctx.account, "eventId": occ[2].id})).await;
    let starts = series_starts(&ctx, &cal, &s.id, from, to).await;
    assert_eq!(starts, ["2026-11-02T09:00:00Z", "2026-11-10T10:00:00Z", "2026-11-23T09:00:00Z"]);

    // Renaming the series (recurrence unchanged) keeps the exception and the deletion.
    let mut rename = input.clone();
    rename["subject"] = json!("Weekly neu");
    let up: Event = call(&ctx.svc, "update_event", json!({"accountId": ctx.account, "eventId": s.id, "input": rename})).await;
    assert_eq!(up.subject, "Weekly neu");
    let after = series_starts(&ctx, &cal, &s.id, from, to).await;
    assert_eq!(after, starts, "series update must not drop exceptions");

    // Changing the pattern really changes it.
    let mut longer = input.clone();
    longer["recurrence"]["occurrences"] = json!(5);
    let _: Event = call(&ctx.svc, "update_event", json!({"accountId": ctx.account, "eventId": s.id, "input": longer})).await;
    assert_eq!(series_starts(&ctx, &cal, &s.id, from, "2027-01-01T00:00:00Z").await.len(), 5);

    // Removing the recurrence turns it into a single event.
    let mut single = input.clone();
    single["recurrence"] = Value::Null;
    let up: Event = call(&ctx.svc, "update_event", json!({"accountId": ctx.account, "eventId": s.id, "input": single})).await;
    assert!(up.recurrence.is_none());
    assert_eq!(up.kind, "singleInstance");

    // Deleting the series removes everything.
    let _: Value = call(&ctx.svc, "delete_event", json!({"accountId": ctx.account, "eventId": s.id})).await;
    assert!(ctx.events(&cal, from, to).await.iter().all(|e| e.id != s.id && e.series_master_id.as_deref() != Some(s.id.as_str())));
    let err = call_err(&ctx.svc, "get_event", json!({"accountId": ctx.account, "eventId": s.id})).await;
    assert!(err.contains("404"), "{err}");
}

#[tokio::test]
async fn respond_event_edge_cases() {
    let ctx = setup_at(now()).await;
    let cal = ctx.default_calendar().await;
    let own = ctx.create_event(&cal, ev_input("Eigener Termin", "2026-11-19T10:00:00", "2026-11-19T11:00:00")).await;
    let err = call_err(
        &ctx.svc,
        "respond_event",
        json!({"accountId": ctx.account, "eventId": own.id, "response": "maybe", "comment": "", "sendResponse": true}),
    )
    .await;
    assert!(err.contains("Ungültige Antwort"), "{err}");
    // Graph refuses answers from the organizer; the error surfaces.
    let err = call_err(
        &ctx.svc,
        "respond_event",
        json!({"accountId": ctx.account, "eventId": own.id, "response": "accept", "comment": "", "sendResponse": false}),
    )
    .await;
    assert!(err.contains("organizer"), "{err}");
    let err = call_err(
        &ctx.svc,
        "respond_event",
        json!({"accountId": ctx.account, "eventId": "AAMkAE-gibt-es-nicht", "response": "decline", "comment": "", "sendResponse": false}),
    )
    .await;
    assert!(err.contains("404"), "{err}");
}

#[tokio::test]
async fn calendars_are_cached_for_offline_use() {
    let ctx = setup_at(now()).await;
    let cals: Vec<Calendar> = call(&ctx.svc, "list_calendars", json!({"accountId": ctx.account})).await;
    assert!(cals[0].is_default, "default calendar first");
    let Ctx { svc, fake, account, dir: _dir, .. } = ctx;
    fake.shutdown();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let offline: Vec<Calendar> = call(&svc, "list_calendars", json!({"accountId": account})).await;
    assert_eq!(offline, cals);
}

#[tokio::test]
async fn shared_calendars_get_own_label_and_colour() {
    let ctx = setup_at(now()).await;
    let cals: Vec<Calendar> = call(&ctx.svc, "list_calendars", json!({"accountId": ctx.account})).await;
    // A colleague's calendar shared in Outlook (not added in the app).
    let shared = cals.iter().find(|c| c.owner.is_some() && !c.id.starts_with("user:")).expect("shared calendar").clone();
    let after: Vec<Calendar> = call(
        &ctx.svc,
        "update_people_calendar",
        json!({"accountId": ctx.account, "calendarId": shared.id, "name": "Chefin", "color": "#D9534F"}),
    )
    .await;
    let c = after.iter().find(|c| c.id == shared.id).unwrap();
    assert_eq!((c.name.as_str(), c.color.as_deref()), ("Chefin", Some("#d9534f")));
    // Kept across a reload from the server …
    let again: Vec<Calendar> = call(&ctx.svc, "list_calendars", json!({"accountId": ctx.account})).await;
    assert_eq!(again.iter().find(|c| c.id == shared.id).unwrap().name, "Chefin");
    // … and an empty name restores the server's.
    let reset: Vec<Calendar> =
        call(&ctx.svc, "update_people_calendar", json!({"accountId": ctx.account, "calendarId": shared.id, "name": ""})).await;
    let c = reset.iter().find(|c| c.id == shared.id).unwrap();
    assert_eq!(c.name, shared.name);
    assert_eq!(c.color.as_deref(), Some("#d9534f"), "colour stays");
}
