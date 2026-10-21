//! Tasks, snooze ("Wiedervorlage"), inbox rules, recurring tasks and the
//! "Heute" view – edge cases through `Service::dispatch`. Times are fixed or
//! set on the fake server; no test waits for the clock.


use chrono::{DateTime, Datelike, Duration, Local, TimeZone, Timelike, Utc, Weekday};
use crate::common::*;
use mailcore::model::*;
use serde_json::{json, Value};

fn local(ts: &str) -> DateTime<Local> {
    DateTime::parse_from_rfc3339(ts).unwrap().with_timezone(&Local)
}

fn graph_due(dt: DateTime<Utc>) -> Value {
    json!({"flagStatus": "flagged", "dueDateTime": {"dateTime": dt.format("%Y-%m-%dT%H:%M:%S").to_string(), "timeZone": "UTC"}})
}

async fn find_in(ctx: &Ctx, wk: &str, subject: &str) -> Option<MessageSummary> {
    let f = ctx.folder(wk).await;
    ctx.sync(&f.id).await;
    ctx.list(&f.id).await.items.into_iter().find(|m| m.subject == subject)
}

#[tokio::test]
async fn month_end_task_returns_to_the_31st() {
    let ctx = setup().await;
    // Anchor: the 31st (date in local time of the machine).
    let due = Local.with_ymd_and_hms(2027, 1, 31, 8, 0, 0).unwrap().with_timezone(&Utc).to_rfc3339();
    let t: MessageSummary = call(
        &ctx.svc,
        "create_task",
        json!({"mailboxId": ctx.account, "subject": "Monatsende", "bodyHtml": "<p>Belege</p>", "due": due, "recurrence": "monthly"}),
    )
    .await;
    let _: Value = call(&ctx.svc, "archive_messages", json!({"mailboxId": ctx.account, "ids": [t.id]})).await;
    let feb = find_in(&ctx, "snoozed", "Monatsende").await.expect("February occurrence");
    let d = local(feb.due_at.as_deref().unwrap());
    assert_eq!((d.month(), d.day()), (2, 28));
    assert!(feb.categories.contains(&"[#d31]".to_string()), "{:?}", feb.categories);
    assert_eq!(feb.recurrence.as_deref(), Some("monthly"));

    let _: Value = call(&ctx.svc, "archive_messages", json!({"mailboxId": ctx.account, "ids": [feb.id]})).await;
    let mar = find_in(&ctx, "snoozed", "Monatsende").await.expect("March occurrence");
    let d = local(mar.due_at.as_deref().unwrap());
    assert_eq!((d.month(), d.day(), d.hour()), (3, 31, 8), "back to the month end at the same wall time");
    assert!(!mar.categories.iter().any(|c| c.starts_with("[#d")), "hint dropped once the day fits");

    // Removing the recurrence also removes a pending hint.
    let _: Value = call(&ctx.svc, "archive_messages", json!({"mailboxId": ctx.account, "ids": [mar.id]})).await;
    let apr = find_in(&ctx, "snoozed", "Monatsende").await.unwrap();
    assert!(apr.categories.contains(&"[#d31]".to_string()));
    let _: Value = call(&ctx.svc, "set_recurrence", json!({"mailboxId": ctx.account, "ids": [apr.id], "recurrence": null})).await;
    let apr = find_in(&ctx, "snoozed", "Monatsende").await.unwrap();
    assert!(apr.recurrence.is_none());
    assert!(!apr.categories.iter().any(|c| c.starts_with("[#")), "{:?}", apr.categories);
}


#[tokio::test]
async fn recurring_task_without_due_anchors_at_now() {
    let ctx = setup().await;
    let t: MessageSummary = call(
        &ctx.svc,
        "create_task",
        json!({"mailboxId": ctx.account, "subject": "Wochenbericht", "bodyHtml": "", "due": null, "recurrence": "weekly"}),
    )
    .await;
    let inbox = ctx.folder("inbox").await;
    assert_eq!(t.folder_id, inbox.id, "no due date: straight into the inbox");
    let before = Utc::now();
    let _: Value = call(&ctx.svc, "archive_messages", json!({"mailboxId": ctx.account, "ids": [t.id]})).await;
    let next = find_in(&ctx, "snoozed", "Wochenbericht").await.expect("next week");
    let due = DateTime::parse_from_rfc3339(next.due_at.as_deref().unwrap()).unwrap().with_timezone(&Utc);
    let diff = due - before;
    assert!(diff > Duration::days(6) && diff <= Duration::days(7) + Duration::minutes(1), "{diff:?}");
}

#[tokio::test]
async fn task_input_validation() {
    let ctx = setup().await;
    let err = |args: Value| {
        let svc = ctx.svc.clone();
        async move { call_err(&svc, "create_task", args).await }
    };
    let m = ctx.account.clone();
    assert!(err(json!({"mailboxId": m, "subject": "x", "recurrence": "hourly"})).await.contains("Unbekannte Wiederholung"));
    assert!(err(json!({"mailboxId": m, "subject": "\t \n", "due": null})).await.contains("Betreff"));
    assert!(err(json!({"mailboxId": m, "subject": "x", "due": "morgen"})).await.contains("Ungültiger Fälligkeitszeitpunkt"));
    let past = (Utc::now() - Duration::minutes(1)).to_rfc3339();
    assert!(err(json!({"mailboxId": m, "subject": "x", "due": past})).await.contains("Vergangenheit"));
    assert!(err(json!({"mailboxId": "gibt-es-nicht", "subject": "x"})).await.len() > 3);

    // Long unicode subject, empty recurrence string = none, whitespace-only due = none.
    let subject = format!("Größe prüfen – 😀 {}", "ä".repeat(300));
    let t: MessageSummary = call(
        &ctx.svc,
        "create_task",
        json!({"mailboxId": m, "subject": format!("  {subject}  "), "bodyHtml": "<p>ß</p>", "due": "  ", "recurrence": ""}),
    )
    .await;
    assert_eq!(t.subject, subject);
    assert!(t.recurrence.is_none() && t.due_at.is_none());
    let raw = ctx.fake.state.message(ME, &t.id).unwrap();
    assert_eq!(raw["subject"], json!(subject));
}

#[tokio::test]
async fn snooze_validation_and_resnooze() {
    let ctx = setup().await;
    let inbox = ctx.folder("inbox").await;
    let m = ctx.list(&inbox.id).await.items[0].clone();
    let snooze = |id: &str, until: String| {
        let svc = ctx.svc.clone();
        let args = json!({"mailboxId": ctx.account, "ids": [id], "until": until});
        async move { svc.dispatch("snooze_messages", args).await }
    };
    let e = snooze(&m.id, "übermorgen".into()).await.unwrap_err().to_string();
    assert!(e.contains("Ungültiger Zeitpunkt"), "{e}");
    let e = snooze(&m.id, (Utc::now() - Duration::seconds(1)).to_rfc3339()).await.unwrap_err().to_string();
    assert!(e.contains("Vergangenheit"), "{e}");
    // Nothing moved by the failed attempts.
    assert!(ctx.list(&inbox.id).await.items.iter().any(|x| x.id == m.id));

    let first = (Utc::now() + Duration::days(2)).to_rfc3339();
    snooze(&m.id, first).await.unwrap();
    let snoozed = ctx.folder("snoozed").await;
    let s = ctx.list(&snoozed.id).await.items.into_iter().find(|x| x.subject == m.subject).unwrap();
    // Snoozing again only changes the time: same message, same folder.
    let second = Utc::now() + Duration::days(5);
    snooze(&s.id, second.to_rfc3339()).await.unwrap();
    let again = ctx.list(&snoozed.id).await.items.into_iter().find(|x| x.subject == m.subject).unwrap();
    assert_eq!(again.id, s.id);
    assert_eq!(again.due_at.as_deref(), Some(second.to_rfc3339_opts(chrono::SecondsFormat::Secs, true).as_str()));
    let raw = ctx.fake.state.message(ME, &s.id).unwrap();
    assert_eq!(raw["flag"]["dueDateTime"]["dateTime"].as_str().unwrap()[..19], second.format("%Y-%m-%dT%H:%M:%S").to_string());
    // Offset notation is accepted and normalised.
    let berlin = (Utc::now() + Duration::days(3)).with_timezone(&chrono::FixedOffset::east_opt(2 * 3600).unwrap());
    snooze(&s.id, berlin.to_rfc3339()).await.unwrap();
    let again = ctx.list(&snoozed.id).await.items.into_iter().find(|x| x.id == s.id).unwrap();
    assert!(again.due_at.as_deref().unwrap().ends_with('Z'));
}

#[tokio::test]
async fn snooze_that_fell_due_while_unreachable_returns_later() {
    let ctx = setup().await;
    let inbox = ctx.folder("inbox").await;
    let m = ctx.list(&inbox.id).await.items[0].clone();
    let until = (Utc::now() + Duration::days(1)).to_rfc3339();
    let _: Value = call(&ctx.svc, "snooze_messages", json!({"mailboxId": ctx.account, "ids": [m.id], "until": until})).await;
    let snoozed = ctx.folder("snoozed").await;
    let s = ctx.list(&snoozed.id).await.items.into_iter().find(|x| x.subject == m.subject).unwrap();
    // The due time passes (set on the server: "time travel").
    assert!(ctx.fake.state.update_message(ME, &s.id, json!({"flag": graph_due(Utc::now() - Duration::hours(3))})));
    // The server is unreachable for this folder during the next round.
    ctx.fake.state.inject_fault(graph_fake::Fault::status(500).on_path(&s.folder_id));
    ctx.svc.sync_all(true).await;
    assert!(ctx.list(&snoozed.id).await.items.iter().any(|x| x.id == s.id), "nothing lost while failing");
    let status = ctx.emitted("sync-status");
    assert_eq!(status.last().unwrap()["state"], "error");
    // Next round: returned to the inbox, unread, flag cleared.
    ctx.svc.sync_all(true).await;
    assert!(!ctx.list(&snoozed.id).await.items.iter().any(|x| x.subject == m.subject));
    let back = ctx.list(&inbox.id).await.items.into_iter().find(|x| x.subject == m.subject).expect("back in inbox");
    assert!(!back.is_read);
    assert_eq!(back.flag, "notFlagged");
    assert!(back.due_at.is_none());
    assert!(ctx.emitted("new-mail").iter().any(|p| p["subject"] == json!(m.subject)));
}

#[tokio::test]
async fn inbox_rules_only_for_own_unread_mails() {
    let ctx = setup().await;
    let inbox = ctx.folder("inbox").await;
    ctx.sync(&inbox.id).await;
    let me_upper = json!({"emailAddress": {"name": "Max", "address": ME.to_uppercase()}});
    let other = json!({"emailAddress": {"name": "Erika", "address": "erika@fabrikam.example"}});
    ctx.fake.state.inject_incoming(ME, "inbox", json!({"subject": "Planung #NÄCHSTEWOCHE", "from": me_upper}));
    ctx.fake.state.inject_incoming(ME, "inbox", json!({"subject": "Fremd #morgen", "from": other}));
    ctx.fake.state.inject_incoming(ME, "inbox", json!({"subject": "Ticket#morgen", "from": me_upper}));
    ctx.fake.state.inject_incoming(ME, "inbox", json!({"subject": "#heute Rückruf", "from": me_upper}));
    let before = Utc::now();
    ctx.sync(&inbox.id).await;
    let in_inbox: Vec<String> = ctx.list(&inbox.id).await.items.into_iter().map(|m| m.subject).collect();
    assert!(in_inbox.contains(&"Fremd #morgen".to_string()), "rules only apply to mails from me");
    assert!(in_inbox.contains(&"Ticket#morgen".to_string()), "token must stand alone");
    assert!(!in_inbox.contains(&"Planung #NÄCHSTEWOCHE".to_string()));
    assert!(!in_inbox.contains(&"#heute Rückruf".to_string()));

    let planned = find_in(&ctx, "snoozed", "Planung #NÄCHSTEWOCHE").await.unwrap();
    let d = local(planned.due_at.as_deref().unwrap());
    assert_eq!(d.weekday(), Weekday::Mon);
    assert_eq!((d.hour(), d.minute()), (8, 0));
    assert!(d.with_timezone(&Utc) > before && d.with_timezone(&Utc) <= before + Duration::days(8));
    let today = find_in(&ctx, "snoozed", "#heute Rückruf").await.unwrap();
    let d = DateTime::parse_from_rfc3339(today.due_at.as_deref().unwrap()).unwrap().with_timezone(&Utc);
    assert!(d > before && d <= before + Duration::hours(18), "{d}");
    let applied = ctx.emitted("rules-applied");
    assert_eq!(applied.last().unwrap()["count"], 2);
}

#[tokio::test]
async fn today_view_due_items_and_exclusions() {
    let ctx = setup().await;
    let inbox = ctx.folder("inbox").await;
    let deleted = ctx.folder("deleteditems").await;
    ctx.sync(&inbox.id).await;
    ctx.sync(&deleted.id).await;
    let yesterday = Utc::now() - Duration::days(1);
    let week_ago = Utc::now() - Duration::days(7);
    ctx.fake.state.inject_incoming(ME, "inbox", json!({"subject": "Überfällig", "flag": graph_due(yesterday)}));
    ctx.fake.state.inject_incoming(ME, "inbox", json!({"subject": "Sehr überfällig", "flag": graph_due(week_ago)}));
    ctx.fake.state.inject_incoming(ME, "inbox", json!({"subject": "Erledigt", "flag": {"flagStatus": "complete",
        "dueDateTime": {"dateTime": yesterday.format("%Y-%m-%dT%H:%M:%S").to_string(), "timeZone": "UTC"}}}));
    ctx.fake.state.inject_incoming(ME, "inbox", json!({"subject": "Nächsten Monat", "flag": graph_due(Utc::now() + Duration::days(30))}));
    ctx.fake.state.inject_incoming(ME, "deleteditems", json!({"subject": "Gelöscht fällig", "flag": graph_due(yesterday)}));
    ctx.sync(&inbox.id).await;
    ctx.sync(&deleted.id).await;

    let today: Value = call(&ctx.svc, "list_today", json!({})).await;
    let due: Vec<&str> = today["due"].as_array().unwrap().iter().map(|m| m["subject"].as_str().unwrap()).collect();
    let pos = |s: &str| due.iter().position(|x| *x == s);
    assert!(pos("Sehr überfällig").unwrap() < pos("Überfällig").unwrap(), "oldest first: {due:?}");
    assert!(pos("Erledigt").is_none(), "completed flags are not due");
    assert!(pos("Nächsten Monat").is_none());
    assert!(pos("Gelöscht fällig").is_none(), "deleted items are excluded");
    // No lanes configured: no todo block.
    assert!(today["todo"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn recurring_task_returned_to_inbox_keeps_its_rhythm() {
    let ctx = setup().await;
    let due = (Utc::now() + Duration::days(1)).to_rfc3339();
    let t: MessageSummary = call(
        &ctx.svc,
        "create_task",
        json!({"mailboxId": ctx.account, "subject": "Täglich lesen", "due": due, "recurrence": "daily"}),
    )
    .await;
    // Due passes on the server; the background round returns it.
    let anchor = Utc::now() - Duration::minutes(90);
    assert!(ctx.fake.state.update_message(ME, &t.id, json!({"flag": graph_due(anchor)})));
    ctx.svc.sync_all(true).await;
    let back = find_in(&ctx, "inbox", "Täglich lesen").await.expect("returned");
    assert!(!back.is_read);
    assert_eq!(back.flag, "flagged", "recurring tasks keep their flag …");
    assert_eq!(back.due_at.as_deref(), Some(anchor.to_rfc3339_opts(chrono::SecondsFormat::Secs, true).as_str()), "… and due time");
    // Done today → tomorrow at the same time, not "now + 1 day".
    let _: Value = call(&ctx.svc, "archive_messages", json!({"mailboxId": ctx.account, "ids": [back.id]})).await;
    let next = find_in(&ctx, "snoozed", "Täglich lesen").await.unwrap();
    let next_due = DateTime::parse_from_rfc3339(next.due_at.as_deref().unwrap()).unwrap().with_timezone(&Utc);
    let expect = Local.from_utc_datetime(&anchor.naive_utc()) + Duration::days(1);
    assert_eq!(next_due.with_timezone(&Local).time().format("%H:%M").to_string(), expect.time().format("%H:%M").to_string());
    assert!(next_due > Utc::now());
}

#[tokio::test]
async fn archive_and_mark_read_of_mail_deleted_on_server_meanwhile() {
    // Exchange's calendar attendant deletes processed meeting responses on its
    // own; the cached copy must still be archivable without a Graph 404.
    let ctx = setup().await;
    let inbox = ctx.folder("inbox").await;
    let items = ctx.list(&inbox.id).await.items;
    let (a, b) = (items[0].clone(), items[1].clone());
    assert!(ctx.fake.state.delete_message(ME, &a.id));
    assert!(ctx.fake.state.delete_message(ME, &b.id));
    let _: Value = call(&ctx.svc, "archive_messages", json!({"mailboxId": ctx.account, "ids": [a.id]})).await;
    let _: Value = call(&ctx.svc, "set_read", json!({"mailboxId": ctx.account, "ids": [b.id], "isRead": true})).await;
    let left = ctx.list(&inbox.id).await.items;
    assert!(!left.iter().any(|x| x.id == a.id || x.id == b.id), "gone mails leave the cache");
}
