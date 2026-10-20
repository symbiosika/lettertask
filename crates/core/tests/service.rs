//! End-to-end tests of the service layer against the fake Graph server,
//! including the real PKCE/loopback login flow.

use std::sync::Arc;

use base64::Engine;
use mailcore::host::NullHost;
use mailcore::model::*;
use mailcore::tokens::MemoryTokenStore;
use mailcore::{Service, ServiceOptions};
use serde_json::{json, Value};

const ME: &str = "max.mustermann@contoso.example";

struct Ctx {
    svc: Arc<Service>,
    host: Arc<NullHost>,
    fake: Option<graph_fake::FakeServer>,
    _dir: tempfile::TempDir,
    account: String,
}

async fn setup() -> Ctx {
    let fake = graph_fake::spawn(graph_fake::FakeOptions {
        seed_demo: true,
        now: chrono::Utc::now(),
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let host = Arc::new(NullHost::default());
    let svc = Service::new(
        ServiceOptions {
            data_dir: dir.path().to_path_buf(),
            graph_base: format!("{}/v1.0", fake.base_url),
            authority: fake.base_url.clone(),
            demo: true,
            token_store: Arc::new(MemoryTokenStore::default()),
        },
        host.clone(),
    )
    .unwrap();
    svc.ensure_demo_account().await.unwrap();
    let accounts: Vec<Account> = call(&svc, "list_accounts", json!({})).await;
    let account = accounts[0].id.clone();
    Ctx {
        svc,
        host,
        fake: Some(fake),
        _dir: dir,
        account,
    }
}

async fn call<T: serde::de::DeserializeOwned>(svc: &Arc<Service>, cmd: &str, args: Value) -> T {
    let v = svc
        .dispatch(cmd, args.clone())
        .await
        .unwrap_or_else(|e| panic!("{cmd}({args}) failed: {e}"));
    serde_json::from_value(v).unwrap_or_else(|e| panic!("{cmd}: cannot decode result: {e}"))
}

async fn call_err(svc: &Arc<Service>, cmd: &str, args: Value) -> String {
    match svc.dispatch(cmd, args).await {
        Ok(v) => panic!("{cmd} unexpectedly succeeded: {v}"),
        Err(e) => e.to_string(),
    }
}

impl Ctx {
    async fn folders(&self, mailbox: &str) -> Vec<Folder> {
        call(&self.svc, "list_folders", json!({"mailboxId": mailbox})).await
    }

    async fn folder(&self, wk: &str) -> Folder {
        self.folders(&self.account)
            .await
            .into_iter()
            .find(|f| f.well_known.as_deref() == Some(wk))
            .unwrap_or_else(|| panic!("folder {wk} missing"))
    }

    async fn list(&self, folder: &str) -> MessagePage {
        call(
            &self.svc,
            "list_messages",
            json!({"mailboxId": self.account, "folderId": folder, "sortBy": "received", "sortDesc": true, "offset": 0, "limit": 500}),
        )
        .await
    }

    async fn sync(&self, folder: &str) -> SyncResult {
        call(&self.svc, "sync_folder", json!({"mailboxId": self.account, "folderId": folder})).await
    }
}

#[tokio::test]
async fn login_creates_account_and_folder_tree() {
    let ctx = setup().await;
    let accounts: Vec<Account> = call(&ctx.svc, "list_accounts", json!({})).await;
    assert_eq!(accounts.len(), 1);
    let acc = &accounts[0];
    assert_eq!(acc.email, ME);
    assert_eq!(acc.mailboxes.len(), 1);
    assert_eq!(acc.mailboxes[0].kind, "primary");

    let folders = ctx.folders(&ctx.account).await;
    let order: Vec<_> = folders.iter().filter_map(|f| f.well_known.clone()).collect();
    assert_eq!(&order[..4], ["inbox", "drafts", "sentitems", "deleteditems"]);
    let projekte = folders.iter().find(|f| f.display_name == "Projekte").expect("Projekte");
    let inbox = ctx.folder("inbox").await;
    assert_eq!(projekte.parent_id.as_deref(), Some(inbox.id.as_str()));
    let alpha = folders.iter().find(|f| f.display_name == "Kunde Alpha").expect("Kunde Alpha");
    assert_eq!(alpha.parent_id.as_deref(), Some(projekte.id.as_str()));
    // Top level folders have no parent (root is hidden).
    assert!(inbox.parent_id.is_none());
}

#[tokio::test]
async fn inbox_is_synced_sorted_and_filterable() {
    let ctx = setup().await;
    let inbox = ctx.folder("inbox").await;
    let page = ctx.list(&inbox.id).await;
    assert!(page.total >= 20, "inbox has {} messages", page.total);
    assert!(page.items.windows(2).all(|w| w[0].received_at >= w[1].received_at));
    assert!(page.unread > 0);

    let unread: MessagePage = call(
        &ctx.svc,
        "list_messages",
        json!({"mailboxId": ctx.account, "folderId": inbox.id, "unreadOnly": true}),
    )
    .await;
    assert!(unread.items.iter().all(|m| !m.is_read));
    assert_eq!(unread.total, page.unread);

    let by_subject: MessagePage = call(
        &ctx.svc,
        "list_messages",
        json!({"mailboxId": ctx.account, "folderId": inbox.id, "sortBy": "subject", "sortDesc": false}),
    )
    .await;
    let subjects: Vec<String> = by_subject.items.iter().map(|m| m.subject.to_lowercase()).collect();
    let mut sorted = subjects.clone();
    sorted.sort();
    assert_eq!(subjects, sorted);

    // A second sync without changes reports nothing.
    let r = ctx.sync(&inbox.id).await;
    assert_eq!((r.added, r.updated, r.removed), (0, 0, 0));
}

#[tokio::test]
async fn message_detail_has_attachments_and_inline_images() {
    let ctx = setup().await;
    let inbox = ctx.folder("inbox").await;
    let page = ctx.list(&inbox.id).await;
    let with_att = page.items.iter().find(|m| m.has_attachments).expect("message with attachment");
    let d: MessageDetail = call(
        &ctx.svc,
        "get_message",
        json!({"mailboxId": ctx.account, "messageId": with_att.id}),
    )
    .await;
    assert!(!d.attachments.is_empty());
    assert!(!d.body_html.is_empty());

    // Find the message with an inline image by scanning bodies.
    let mut found_inline = false;
    for m in &page.items {
        let d: MessageDetail = call(&ctx.svc, "get_message", json!({"mailboxId": ctx.account, "messageId": m.id})).await;
        assert!(!d.body_html.contains("cid:"), "unresolved cid in {}", m.subject);
        if d.body_html.contains("data:image/png;base64,") {
            found_inline = true;
        }
    }
    assert!(found_inline, "no inline image found in demo data");

    // Attachment download ("save" goes to the NullHost temp dir).
    let path: Option<String> = call(
        &ctx.svc,
        "download_attachment",
        json!({"mailboxId": ctx.account, "messageId": with_att.id, "attachmentId": d.attachments[0].id, "action": "save"}),
    )
    .await;
    let bytes = std::fs::read(path.unwrap()).unwrap();
    assert_eq!(bytes.len() as i64, d.attachments[0].size);

    let src: String = call(&ctx.svc, "get_message_source", json!({"mailboxId": ctx.account, "messageId": with_att.id})).await;
    assert!(src.to_lowercase().contains("subject:"));
}

#[tokio::test]
async fn read_flag_categories_roundtrip_to_server() {
    let ctx = setup().await;
    let inbox = ctx.folder("inbox").await;
    let page = ctx.list(&inbox.id).await;
    let m = page.items.iter().find(|m| !m.is_read).unwrap().clone();
    let ids = json!([m.id]);
    let _: Value = call(&ctx.svc, "set_read", json!({"mailboxId": ctx.account, "ids": ids, "isRead": true})).await;
    let _: Value = call(&ctx.svc, "set_flag", json!({"mailboxId": ctx.account, "ids": ids, "flag": "flagged"})).await;
    let _: Value = call(
        &ctx.svc,
        "set_categories",
        json!({"mailboxId": ctx.account, "ids": ids, "categories": ["Wichtig", "Info"]}),
    )
    .await;
    // Local cache updated immediately…
    let after = ctx.list(&inbox.id).await;
    let cached = after.items.iter().find(|x| x.id == m.id).unwrap();
    assert!(cached.is_read);
    assert_eq!(cached.flag, "flagged");
    assert_eq!(cached.categories, ["Wichtig", "Info"]);
    assert_eq!(after.unread, page.unread - 1);
    let f = ctx.folder("inbox").await;
    assert_eq!(f.unread_count, inbox.unread_count - 1);
    // …and the server agrees.
    let d: MessageDetail = call(&ctx.svc, "get_message", json!({"mailboxId": ctx.account, "messageId": m.id})).await;
    assert!(d.summary.is_read);
    assert_eq!(d.summary.flag, "flagged");
    assert_eq!(d.summary.categories, ["Wichtig", "Info"]);
    assert!(ctx.host.events.lock().unwrap().iter().any(|(e, _)| e == "mail-changed"));
}

#[tokio::test]
async fn move_copy_delete_and_folder_management() {
    let ctx = setup().await;
    let inbox = ctx.folder("inbox").await;
    let page = ctx.list(&inbox.id).await;
    let target: Folder = call(
        &ctx.svc,
        "create_folder",
        json!({"mailboxId": ctx.account, "parentId": inbox.id, "name": "Testordner"}),
    )
    .await;
    assert_eq!(target.parent_id.as_deref(), Some(inbox.id.as_str()));

    let m1 = page.items[0].clone();
    let m2 = page.items[1].clone();
    let _: Value = call(
        &ctx.svc,
        "move_messages",
        json!({"mailboxId": ctx.account, "ids": [m1.id], "destFolderId": target.id}),
    )
    .await;
    let _: Value = call(
        &ctx.svc,
        "copy_messages",
        json!({"mailboxId": ctx.account, "ids": [m2.id], "destFolderId": target.id}),
    )
    .await;
    let in_target = ctx.list(&target.id).await;
    assert_eq!(in_target.total, 2);
    assert!(in_target.items.iter().any(|m| m.subject == m1.subject));
    let in_inbox = ctx.list(&inbox.id).await;
    assert!(!in_inbox.items.iter().any(|m| m.id == m1.id));
    assert!(in_inbox.items.iter().any(|m| m.id == m2.id));

    // Delta after a move must not resurrect or duplicate anything.
    ctx.sync(&inbox.id).await;
    ctx.sync(&target.id).await;
    assert_eq!(ctx.list(&target.id).await.total, 2);
    assert_eq!(ctx.list(&inbox.id).await.total, in_inbox.total);

    // Delete → Deleted Items; delete again → gone.
    let victim = ctx.list(&target.id).await.items[0].clone();
    let _: Value = call(&ctx.svc, "delete_messages", json!({"mailboxId": ctx.account, "ids": [victim.id]})).await;
    let deleted = ctx.folder("deleteditems").await;
    ctx.sync(&deleted.id).await;
    let in_deleted = ctx.list(&deleted.id).await;
    let moved = in_deleted.items.iter().find(|m| m.subject == victim.subject).expect("in deleted items").clone();
    let _: Value = call(&ctx.svc, "delete_messages", json!({"mailboxId": ctx.account, "ids": [moved.id]})).await;
    ctx.sync(&deleted.id).await;
    assert!(!ctx.list(&deleted.id).await.items.iter().any(|m| m.id == moved.id));

    let renamed: Folder = call(
        &ctx.svc,
        "rename_folder",
        json!({"mailboxId": ctx.account, "folderId": target.id, "name": "Umbenannt"}),
    )
    .await;
    assert_eq!(renamed.display_name, "Umbenannt");
    let err = call_err(&ctx.svc, "delete_folder", json!({"mailboxId": ctx.account, "folderId": inbox.id})).await;
    assert!(err.contains("Standardordner"));
    let _: Value = call(&ctx.svc, "delete_folder", json!({"mailboxId": ctx.account, "folderId": target.id})).await;
    // Like Outlook, a deleted folder ends up below "Gelöschte Elemente".
    let folders = ctx.folders(&ctx.account).await;
    if let Some(f) = folders.iter().find(|f| f.id == target.id) {
        assert_eq!(f.parent_id.as_deref(), Some(deleted.id.as_str()));
    }

    let _: Value = call(&ctx.svc, "mark_folder_read", json!({"mailboxId": ctx.account, "folderId": inbox.id})).await;
    assert_eq!(ctx.folder("inbox").await.unread_count, 0);
    assert_eq!(ctx.list(&inbox.id).await.unread, 0);
}

#[tokio::test]
async fn delta_picks_up_server_side_changes() {
    let ctx = setup().await;
    let inbox = ctx.folder("inbox").await;
    let before = ctx.list(&inbox.id).await;
    let fake = ctx.fake.as_ref().unwrap();
    let token = fake.issue_token();
    let http = reqwest::Client::new();
    let victim = &before.items[3];
    // Another client deletes one message and marks another unread.
    http.delete(format!("{}/v1.0/me/messages/{}", fake.base_url, urlencoding::encode(&victim.id)))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let read_one = before.items.iter().find(|m| m.is_read && m.id != victim.id).unwrap();
    http.patch(format!("{}/v1.0/me/messages/{}", fake.base_url, urlencoding::encode(&read_one.id)))
        .bearer_auth(&token)
        .json(&json!({"isRead": false}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    // …and a new mail arrives (sent to ourselves).
    http.post(format!("{}/v1.0/me/sendMail", fake.base_url))
        .bearer_auth(&token)
        .json(&json!({"message": {"subject": "Neu per Delta", "body": {"contentType": "text", "content": "x"},
            "toRecipients": [{"emailAddress": {"address": ME}}]}, "saveToSentItems": true}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();

    let r = ctx.sync(&inbox.id).await;
    assert_eq!(r.removed, 1, "{r:?}");
    assert_eq!(r.added, 1, "{r:?}");
    assert!(r.updated >= 1, "{r:?}");
    let after = ctx.list(&inbox.id).await;
    assert!(!after.items.iter().any(|m| m.id == victim.id));
    assert_eq!(after.items[0].subject, "Neu per Delta");
    assert!(!after.items.iter().find(|m| m.id == read_one.id).unwrap().is_read);
    // Not quiet → new-mail event emitted.
    assert!(ctx.host.events.lock().unwrap().iter().any(|(e, _)| e == "new-mail"));
}

#[tokio::test]
async fn compose_send_with_large_attachment_and_reply() {
    let ctx = setup().await;
    let _: Config = call(
        &ctx.svc,
        "save_config",
        json!({"config": {"signatureHtml": "<p>Viele Grüße<br>Max</p>", "syncIntervalSecs": 60}}),
    )
    .await;
    let draft: Draft = call(&ctx.svc, "create_draft", json!({"mailboxId": ctx.account, "mode": "new"})).await;
    assert!(draft.id.is_none());
    assert!(draft.body_html.contains("Viele Grüße"));

    let big = vec![b'A'; 4 * 1024 * 1024 + 123];
    let b64 = |b: &[u8]| base64::engine::general_purpose::STANDARD.encode(b);
    let input = json!({
        "id": null, "mailboxId": ctx.account, "mode": "new",
        "to": [{"name": "Max", "address": ME}], "cc": [], "bcc": [],
        "subject": "Test mit Anhang", "bodyHtml": "<p>Hallo</p>", "importance": "high",
        "requestReadReceipt": false,
        "newAttachments": [
            {"name": "klein.txt", "contentType": "text/plain", "contentBase64": b64(b"hallo")},
            {"name": "gross.bin", "contentType": "application/octet-stream", "contentBase64": b64(&big)}
        ],
        "removedAttachmentIds": []
    });
    // Save first: draft appears in Drafts with both attachments.
    let saved: Draft = call(&ctx.svc, "save_draft", json!({"draft": input})).await;
    let id = saved.id.clone().unwrap();
    assert_eq!(saved.attachments.len(), 2);
    assert!(saved.attachments.iter().any(|a| a.name == "gross.bin" && a.size == big.len() as i64));
    let drafts = ctx.folder("drafts").await;
    assert!(ctx.list(&drafts.id).await.items.iter().any(|m| m.id == id));

    // Remove the small one and send.
    let small = saved.attachments.iter().find(|a| a.name == "klein.txt").unwrap();
    let mut send = input.clone();
    send["id"] = json!(id);
    send["newAttachments"] = json!([]);
    send["removedAttachmentIds"] = json!([small.id]);
    let _: Value = call(&ctx.svc, "send_draft", json!({"draft": send})).await;
    assert!(!ctx.list(&drafts.id).await.items.iter().any(|m| m.id == id));
    let sent = ctx.folder("sentitems").await;
    assert!(ctx.list(&sent.id).await.items.iter().any(|m| m.subject == "Test mit Anhang"));
    let inbox = ctx.folder("inbox").await;
    let received = ctx
        .list(&inbox.id)
        .await
        .items
        .into_iter()
        .find(|m| m.subject == "Test mit Anhang")
        .expect("delivered to own inbox");
    assert!(received.has_attachments);
    assert_eq!(received.importance, "high");
    let d: MessageDetail = call(&ctx.svc, "get_message", json!({"mailboxId": ctx.account, "messageId": received.id})).await;
    assert_eq!(d.attachments.len(), 1);
    assert_eq!(d.attachments[0].name, "gross.bin");

    // Reply: recipients and subject prefilled, signature on top of the quote.
    let reply: Draft = call(
        &ctx.svc,
        "create_draft",
        json!({"mailboxId": ctx.account, "mode": "reply", "sourceMessageId": received.id}),
    )
    .await;
    assert!(reply.id.is_some());
    assert!(reply.subject.starts_with("AW:") || reply.subject.starts_with("RE:"), "{}", reply.subject);
    assert_eq!(reply.to[0].address, ME);
    let sig = reply.body_html.find("Viele Grüße").unwrap();
    let quote = reply.body_html.find("Hallo").unwrap();
    assert!(sig < quote);
    let _: Value = call(&ctx.svc, "discard_draft", json!({"mailboxId": ctx.account, "draftId": reply.id})).await;

    // Validation.
    let mut bad = input.clone();
    bad["to"] = json!([]);
    bad["newAttachments"] = json!([]);
    assert!(call_err(&ctx.svc, "send_draft", json!({"draft": bad})).await.contains("Empfänger"));

    let sugg: Vec<Address> = call(&ctx.svc, "suggest_recipients", json!({"mailboxId": ctx.account, "query": "max"})).await;
    assert!(sugg.iter().any(|a| a.address == ME));
}

#[tokio::test]
async fn search_and_load_older() {
    let ctx = setup().await;
    let inbox = ctx.folder("inbox").await;
    let page = ctx.list(&inbox.id).await;
    let word = page.items[5].subject.split_whitespace().max_by_key(|w| w.len()).unwrap().to_string();
    let hits: Vec<MessageSummary> = call(
        &ctx.svc,
        "search_messages",
        json!({"mailboxId": ctx.account, "text": word, "folderId": null}),
    )
    .await;
    assert!(hits.iter().any(|m| m.id == page.items[5].id), "search for {word}");
    let none: Vec<MessageSummary> = call(
        &ctx.svc,
        "search_messages",
        json!({"mailboxId": ctx.account, "text": "xyzzy-gibt-es-nicht", "folderId": inbox.id}),
    )
    .await;
    assert!(none.is_empty());
    let n: i64 = call(&ctx.svc, "load_older", json!({"mailboxId": ctx.account, "folderId": inbox.id})).await;
    let after = ctx.list(&inbox.id).await;
    assert_eq!(after.total, page.total + n);
    assert!(!after.can_load_older || n > 0);
}

#[tokio::test]
async fn categories_crud() {
    let ctx = setup().await;
    let cats: Vec<Category> = call(&ctx.svc, "list_categories", json!({"accountId": ctx.account})).await;
    assert!(cats.iter().any(|c| c.display_name == "Wichtig" && c.color == "preset0"));
    let c: Category = call(
        &ctx.svc,
        "create_category",
        json!({"accountId": ctx.account, "displayName": "Kunde Beta", "color": "preset9"}),
    )
    .await;
    let dup = call_err(
        &ctx.svc,
        "create_category",
        json!({"accountId": ctx.account, "displayName": "Kunde Beta", "color": "preset9"}),
    )
    .await;
    assert!(dup.contains("existiert bereits"), "{dup}");
    let u: Category = call(&ctx.svc, "update_category", json!({"accountId": ctx.account, "id": c.id, "color": "preset2"})).await;
    assert_eq!(u.color, "preset2");
    let _: Value = call(&ctx.svc, "delete_category", json!({"accountId": ctx.account, "id": c.id})).await;
    let cats: Vec<Category> = call(&ctx.svc, "list_categories", json!({"accountId": ctx.account})).await;
    assert!(!cats.iter().any(|x| x.id == c.id));
    assert!(ctx.host.events.lock().unwrap().iter().any(|(e, _)| e == "categories-changed"));
}

#[tokio::test]
async fn calendar_events_lifecycle() {
    let ctx = setup().await;
    let cals: Vec<Calendar> = call(&ctx.svc, "list_calendars", json!({"accountId": ctx.account})).await;
    assert!(cals[0].is_default);
    let default = cals[0].id.clone();
    let holidays = cals.iter().find(|c| !c.can_edit).expect("read-only holiday calendar");
    let ids: Vec<String> = cals.iter().map(|c| c.id.clone()).collect();

    let now = chrono::Utc::now();
    let start = (now - chrono::Duration::days(7)).to_rfc3339();
    let end = (now + chrono::Duration::days(14)).to_rfc3339();
    let events: Vec<Event> = call(
        &ctx.svc,
        "list_events",
        json!({"accountId": ctx.account, "calendarIds": ids, "start": start, "end": end}),
    )
    .await;
    assert!(events.len() >= 10, "{} events", events.len());
    assert!(events.iter().any(|e| e.kind == "occurrence" && e.series_master_id.is_some()));
    assert!(events.iter().all(|e| e.start.ends_with('Z')));

    // Create in winter (CET, UTC+1) for deterministic conversion.
    let input = json!({
        "subject": "Projektmeeting", "start": "2027-01-15T10:00:00", "end": "2027-01-15T11:30:00",
        "timeZone": "Europe/Berlin", "isAllDay": false, "location": "Raum 1", "bodyHtml": "<p>Agenda</p>",
        "attendees": [{"name": "Erika", "address": "erika@contoso.example", "type": "required"}],
        "showAs": "busy", "categories": ["Wichtig"], "isReminderOn": true, "reminderMinutes": 15,
        "isOnlineMeeting": true, "recurrence": null, "sensitivity": "normal", "importance": "normal"
    });
    let ev: Event = call(
        &ctx.svc,
        "create_event",
        json!({"accountId": ctx.account, "calendarId": default, "input": input}),
    )
    .await;
    assert_eq!(ev.start, "2027-01-15T09:00:00Z");
    assert_eq!(ev.end, "2027-01-15T10:30:00Z");
    assert!(ev.online_meeting_url.is_some());
    assert_eq!(ev.attendees.len(), 1);

    let detail: EventDetail = call(&ctx.svc, "get_event", json!({"accountId": ctx.account, "eventId": ev.id})).await;
    assert!(detail.body_html.contains("Agenda"));
    assert_eq!(detail.event.calendar_id, default);

    let mut changed = input.clone();
    changed["subject"] = json!("Projektmeeting (verschoben)");
    changed["start"] = json!("2027-01-15T14:00:00");
    changed["end"] = json!("2027-01-15T15:00:00");
    let up: Event = call(
        &ctx.svc,
        "update_event",
        json!({"accountId": ctx.account, "eventId": ev.id, "input": changed}),
    )
    .await;
    assert_eq!(up.subject, "Projektmeeting (verschoben)");
    assert_eq!(up.start, "2027-01-15T13:00:00Z");

    // Weekly series every Monday and Wednesday, 4 occurrences.
    let mut series = input.clone();
    series["subject"] = json!("Serie");
    series["start"] = json!("2027-02-01T09:00:00");
    series["end"] = json!("2027-02-01T09:30:00");
    series["attendees"] = json!([]);
    series["recurrence"] = json!({"pattern": "weekly", "interval": 1, "daysOfWeek": ["monday", "wednesday"],
        "dayOfMonth": 0, "month": 0, "startDate": "2027-02-01", "endDate": null, "occurrences": 4});
    let s: Event = call(&ctx.svc, "create_event", json!({"accountId": ctx.account, "calendarId": default, "input": series})).await;
    let occ: Vec<Event> = call(
        &ctx.svc,
        "list_events",
        json!({"accountId": ctx.account, "calendarIds": [default], "start": "2027-01-31T00:00:00Z", "end": "2027-03-01T00:00:00Z"}),
    )
    .await;
    let n = occ.iter().filter(|e| e.series_master_id.as_deref() == Some(s.id.as_str())).count();
    assert_eq!(n, 4);

    // Respond to an invitation (an event where we are not organizer).
    let invite = events.iter().find(|e| !e.is_organizer && e.kind != "occurrence").expect("invitation in demo data");
    let _: Value = call(
        &ctx.svc,
        "respond_event",
        json!({"accountId": ctx.account, "eventId": invite.id, "response": "accept", "comment": "Passt", "sendResponse": true}),
    )
    .await;
    let after: EventDetail = call(&ctx.svc, "get_event", json!({"accountId": ctx.account, "eventId": invite.id})).await;
    assert_eq!(after.event.response_status, "accepted");

    let err = call_err(
        &ctx.svc,
        "create_event",
        json!({"accountId": ctx.account, "calendarId": holidays.id, "input": input}),
    )
    .await;
    assert!(!err.is_empty());

    let _: Value = call(&ctx.svc, "delete_event", json!({"accountId": ctx.account, "eventId": ev.id})).await;
    assert!(ctx.svc.dispatch("get_event", json!({"accountId": ctx.account, "eventId": ev.id})).await.is_err());
}

#[tokio::test]
async fn auto_reply_roundtrip() {
    let ctx = setup().await;
    let a: AutoReply = call(&ctx.svc, "get_auto_reply", json!({"accountId": ctx.account})).await;
    assert_eq!(a.status, "disabled");
    let set: AutoReply = call(
        &ctx.svc,
        "set_auto_reply",
        json!({"accountId": ctx.account, "autoReply": {
            "status": "scheduled", "internalReplyMessage": "Bin im Urlaub", "externalReplyMessage": "Out of office",
            "scheduledStart": "2026-12-20T08:00:00Z", "scheduledEnd": "2027-01-06T17:00:00Z", "externalAudience": "all"}}),
    )
    .await;
    assert_eq!(set.status, "scheduled");
    assert_eq!(set.internal_reply_message, "Bin im Urlaub");
    assert_eq!(set.scheduled_start.as_deref(), Some("2026-12-20T08:00:00Z"));
}

#[tokio::test]
async fn works_offline_from_cache() {
    let mut ctx = setup().await;
    let inbox = ctx.folder("inbox").await;
    let page = ctx.list(&inbox.id).await;
    let m = &page.items[0];
    let online: MessageDetail = call(&ctx.svc, "get_message", json!({"mailboxId": ctx.account, "messageId": m.id})).await;
    ctx.fake.take().unwrap().shutdown();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    // Cached list and body still available.
    assert_eq!(ctx.list(&inbox.id).await.total, page.total);
    let offline: MessageDetail = call(&ctx.svc, "get_message", json!({"mailboxId": ctx.account, "messageId": m.id})).await;
    assert_eq!(offline.body_html, online.body_html);
    // Sync reports offline instead of failing hard.
    ctx.svc.sync_all(false).await;
    let events = ctx.host.events.lock().unwrap();
    let last = events.iter().rev().find(|(e, _)| e == "sync-status").unwrap();
    assert_eq!(last.1["state"], "offline");
}

#[tokio::test]
async fn unknown_commands_and_bad_args_are_errors() {
    let ctx = setup().await;
    assert!(call_err(&ctx.svc, "nope", json!({})).await.contains("Unbekannter Befehl"));
    assert!(call_err(&ctx.svc, "list_folders", json!({})).await.contains("mailboxId"));
    let err = call_err(&ctx.svc, "open_url", json!({"url": "file:///etc/passwd"})).await;
    assert!(err.contains("http"));
}

#[tokio::test]
async fn refreshes_expired_tokens_and_retries_throttling() {
    let ctx = setup().await;
    let fake = ctx.fake.as_ref().unwrap();
    let inbox = ctx.folder("inbox").await;
    fake.state.expire_tokens();
    let r = ctx.svc.dispatch("sync_folders", json!({"mailboxId": ctx.account})).await;
    assert!(r.is_ok(), "401 must trigger a token refresh: {r:?}");
    fake.state.fail_next(429, 2);
    let r = ctx.svc.dispatch("sync_folder", json!({"mailboxId": ctx.account, "folderId": inbox.id})).await;
    assert!(r.is_ok(), "429 must be retried: {r:?}");
}

#[tokio::test]
async fn meeting_invitation_links_to_event() {
    let ctx = setup().await;
    let inbox = ctx.folder("inbox").await;
    let page = ctx.list(&inbox.id).await;
    let mut found = None;
    for m in &page.items {
        let d: MessageDetail = call(&ctx.svc, "get_message", json!({"mailboxId": ctx.account, "messageId": m.id})).await;
        if d.is_meeting_request {
            found = Some(d);
            break;
        }
    }
    let d = found.expect("invitation in demo inbox");
    let event_id = d.event_id.expect("event id of invitation");
    let ev: EventDetail = call(&ctx.svc, "get_event", json!({"accountId": ctx.account, "eventId": event_id})).await;
    assert!(!ev.event.is_organizer);
    let _: Value = call(
        &ctx.svc,
        "respond_event",
        json!({"accountId": ctx.account, "eventId": event_id, "response": "tentativelyAccept", "comment": "", "sendResponse": true}),
    )
    .await;
    let ev: EventDetail = call(&ctx.svc, "get_event", json!({"accountId": ctx.account, "eventId": event_id})).await;
    assert_eq!(ev.event.response_status, "tentativelyAccepted");
}

#[tokio::test]
async fn config_has_snooze_defaults() {
    let ctx = setup().await;
    let c: Config = call(&ctx.svc, "get_config", json!({})).await;
    assert_eq!(c.snooze_options.len(), 6);
    assert_eq!(c.snooze_options[0].rule, "tomorrow@08:00");
    assert!(c.show_date_separators);
    // Saving an empty list restores the defaults; custom lists persist.
    let mut c2 = c.clone();
    c2.snooze_options = vec![SnoozeOption { label: "In 3 Tagen".into(), rule: "days:3@09:00".into() }];
    let saved: Config = call(&ctx.svc, "save_config", json!({"config": c2})).await;
    assert_eq!(saved.snooze_options.len(), 1);
    let mut c3 = saved.clone();
    c3.snooze_options.clear();
    let saved: Config = call(&ctx.svc, "save_config", json!({"config": c3})).await;
    assert_eq!(saved.snooze_options.len(), 6);
}

#[tokio::test]
async fn archive_moves_to_archive_folder() {
    let ctx = setup().await;
    let inbox = ctx.folder("inbox").await;
    let archive = ctx.folder("archive").await;
    let m = ctx.list(&inbox.id).await.items[0].clone();
    let _: Value = call(&ctx.svc, "archive_messages", json!({"mailboxId": ctx.account, "ids": [m.id]})).await;
    assert!(!ctx.list(&inbox.id).await.items.iter().any(|x| x.id == m.id));
    assert!(ctx.list(&archive.id).await.items.iter().any(|x| x.subject == m.subject));
}

#[tokio::test]
async fn snooze_and_return_to_inbox() {
    let ctx = setup().await;
    let inbox = ctx.folder("inbox").await;
    let page = ctx.list(&inbox.id).await;
    let a = page.items[0].clone();
    let b = page.items[1].clone();
    let until = (chrono::Utc::now() + chrono::Duration::seconds(3)).to_rfc3339();
    let later = (chrono::Utc::now() + chrono::Duration::days(1)).to_rfc3339();
    let _: Value = call(&ctx.svc, "snooze_messages", json!({"mailboxId": ctx.account, "ids": [a.id], "until": until})).await;
    let _: Value = call(&ctx.svc, "snooze_messages", json!({"mailboxId": ctx.account, "ids": [b.id], "until": later})).await;

    // Folder "Wiedervorlage" exists, sorted right after the inbox.
    let folders = ctx.folders(&ctx.account).await;
    assert_eq!(folders[1].well_known.as_deref(), Some("snoozed"));
    assert_eq!(folders[1].display_name, "Wiedervorlage");
    let snoozed = folders[1].clone();
    let in_snooze = ctx.list(&snoozed.id).await;
    assert_eq!(in_snooze.total, 2);
    assert!(in_snooze.items.iter().all(|m| m.due_at.is_some()));
    assert!(!ctx.list(&inbox.id).await.items.iter().any(|m| m.id == a.id || m.id == b.id));

    // A fresh sync of the folder keeps the due times (they live on the server).
    let _: Value = ctx.svc.dispatch("set_active_folder", json!({"mailboxId": ctx.account, "folderId": snoozed.id})).await.unwrap();
    // Time passes → background sync returns only the due one, unread.
    tokio::time::sleep(std::time::Duration::from_secs(4)).await;
    ctx.svc.sync_all(true).await;
    let back = ctx.list(&inbox.id).await;
    let returned = back.items.iter().find(|m| m.subject == a.subject).expect("returned to inbox");
    assert!(!returned.is_read);
    assert_eq!(returned.flag, "notFlagged");
    assert!(returned.due_at.is_none());
    assert_eq!(ctx.list(&snoozed.id).await.total, 1);
    assert!(ctx.host.events.lock().unwrap().iter().any(|(e, p)| e == "new-mail" && p["subject"] == a.subject));

    // Manual unsnooze.
    let still = ctx.list(&snoozed.id).await.items[0].clone();
    let _: Value = call(&ctx.svc, "unsnooze_messages", json!({"mailboxId": ctx.account, "ids": [still.id]})).await;
    assert_eq!(ctx.list(&snoozed.id).await.total, 0);
    assert!(ctx.list(&inbox.id).await.items.iter().any(|m| m.subject == b.subject && !m.is_read));

    // Past times are rejected.
    let past = (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
    let err = call_err(&ctx.svc, "snooze_messages", json!({"mailboxId": ctx.account, "ids": [a.id], "until": past})).await;
    assert!(err.contains("Vergangenheit"));
}

#[tokio::test]
async fn search_by_field_and_attachments() {
    let ctx = setup().await;
    let inbox = ctx.folder("inbox").await;
    let page = ctx.list(&inbox.id).await;
    let m = page.items.iter().find(|m| m.has_attachments).unwrap().clone();
    let sender = m.from.clone().unwrap();
    let last_name = sender.name.split_whitespace().last().unwrap().to_string();
    let by_from: Vec<MessageSummary> = call(
        &ctx.svc,
        "search_messages",
        json!({"mailboxId": ctx.account, "text": last_name, "folderId": null, "field": "from"}),
    )
    .await;
    assert!(!by_from.is_empty());
    assert!(by_from.iter().all(|x| x.from.as_ref().unwrap().name.contains(&last_name)
        || x.from.as_ref().unwrap().address.to_lowercase().contains(&last_name.to_lowercase())));
    let with_att: Vec<MessageSummary> = call(
        &ctx.svc,
        "search_messages",
        json!({"mailboxId": ctx.account, "text": last_name, "folderId": null, "field": "from", "hasAttachments": true}),
    )
    .await;
    assert!(with_att.iter().all(|x| x.has_attachments));
    assert!(with_att.iter().any(|x| x.id == m.id));
    let word = m.subject.split_whitespace().max_by_key(|w| w.len()).unwrap().to_string();
    let by_subject: Vec<MessageSummary> = call(
        &ctx.svc,
        "search_messages",
        json!({"mailboxId": ctx.account, "text": word, "folderId": inbox.id, "field": "subject"}),
    )
    .await;
    assert!(by_subject.iter().any(|x| x.id == m.id));
    let wrong_field: Vec<MessageSummary> = call(
        &ctx.svc,
        "search_messages",
        json!({"mailboxId": ctx.account, "text": word, "folderId": inbox.id, "field": "to"}),
    )
    .await;
    assert!(!wrong_field.iter().any(|x| x.id == m.id));
}

#[tokio::test]
async fn reply_keeps_inline_images_editable_and_embedded() {
    let ctx = setup().await;
    let inbox = ctx.folder("inbox").await;
    let page = ctx.list(&inbox.id).await;
    let mut source = None;
    for m in &page.items {
        let d: MessageDetail = call(&ctx.svc, "get_message", json!({"mailboxId": ctx.account, "messageId": m.id})).await;
        if d.body_html.contains("data:image/png;base64,") {
            source = Some(m.clone());
            break;
        }
    }
    let source = source.expect("message with inline image");
    let reply: Draft = call(
        &ctx.svc,
        "create_draft",
        json!({"mailboxId": ctx.account, "mode": "reply", "sourceMessageId": source.id}),
    )
    .await;
    // Editor sees a renderable image, no broken cid: and no attachment chip for it.
    assert!(!reply.body_html.contains("cid:"));
    assert!(reply.body_html.contains("#cid="));
    assert!(reply.attachments.iter().all(|a| !a.is_inline));
    // Saving converts back: the server draft references the inline attachment again.
    let input = json!({"id": reply.id, "mailboxId": ctx.account, "mode": "reply", "to": reply.to,
        "cc": [], "bcc": [], "subject": reply.subject, "bodyHtml": format!("<p>Danke!</p>{}", reply.body_html),
        "importance": "normal", "requestReadReceipt": false, "newAttachments": [], "removedAttachmentIds": []});
    let _: Draft = call(&ctx.svc, "save_draft", json!({"draft": input})).await;
    let fake = ctx.fake.as_ref().unwrap();
    let raw = fake.state.message("me", reply.id.as_deref().unwrap()).expect("draft on server");
    let body = raw["body"]["content"].as_str().unwrap();
    assert!(body.contains("cid:"), "server body must use cid: references");
    assert!(!body.contains("#cid="));
    assert!(!body.contains("data:image/png"));
}
