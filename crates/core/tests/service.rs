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
    let _: SignatureSettings = call(
        &ctx.svc,
        "save_signatures",
        json!({"accountId": ctx.account, "settings": {"signatures": [{"id": "", "name": "Standard", "html": "<p>Viele Grüße<br>Max</p>"}],
            "defaultId": null, "useOnNew": true, "useOnReply": true, "useOnForward": false}}),
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
    assert_eq!(c.snooze_options.len(), 8);
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
    assert_eq!(saved.snooze_options.len(), 8);
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

#[tokio::test]
async fn signatures_per_account_and_mode() {
    let ctx = setup().await;
    let empty: SignatureSettings = call(&ctx.svc, "get_signatures", json!({"accountId": ctx.account})).await;
    assert!(empty.signatures.is_empty());
    assert!(empty.use_on_new && empty.use_on_reply && !empty.use_on_forward);
    let saved: SignatureSettings = call(
        &ctx.svc,
        "save_signatures",
        json!({"accountId": ctx.account, "settings": {"signatures": [
            {"id": "", "name": "Kurz", "html": "<p>MM</p>"},
            {"id": "", "name": "Lang", "html": "<p>Max Mustermann<br>Contoso GmbH</p>"}
        ], "defaultId": null, "useOnNew": true, "useOnReply": true, "useOnForward": false}}),
    )
    .await;
    assert_eq!(saved.signatures.len(), 2);
    assert!(saved.signatures.iter().all(|s| !s.id.is_empty()));
    // No valid default given → the first one becomes default.
    assert_eq!(saved.default_id.as_deref(), Some(saved.signatures[0].id.as_str()));
    let mut settings = saved.clone();
    settings.default_id = Some(saved.signatures[1].id.clone());
    let saved: SignatureSettings = call(&ctx.svc, "save_signatures", json!({"accountId": ctx.account, "settings": settings})).await;
    let lang = saved.signatures[1].id.clone();

    let new: Draft = call(&ctx.svc, "create_draft", json!({"mailboxId": ctx.account, "mode": "new"})).await;
    assert!(new.body_html.contains("Contoso GmbH"));
    assert!(new.body_html.contains(&format!("data-signature-id=\"{lang}\"")));

    let inbox = ctx.folder("inbox").await;
    let src = ctx.list(&inbox.id).await.items[0].clone();
    let reply: Draft = call(&ctx.svc, "create_draft", json!({"mailboxId": ctx.account, "mode": "reply", "sourceMessageId": src.id})).await;
    assert!(reply.body_html.contains("class=\"signature\""));
    let fwd: Draft = call(&ctx.svc, "create_draft", json!({"mailboxId": ctx.account, "mode": "forward", "sourceMessageId": src.id})).await;
    assert!(!fwd.body_html.contains("class=\"signature\""), "no signature on forwards by default");

    // Flags are honoured.
    let mut s2 = saved.clone();
    s2.use_on_new = false;
    s2.use_on_forward = true;
    let _: SignatureSettings = call(&ctx.svc, "save_signatures", json!({"accountId": ctx.account, "settings": s2})).await;
    let new: Draft = call(&ctx.svc, "create_draft", json!({"mailboxId": ctx.account, "mode": "new"})).await;
    assert!(!new.body_html.contains("signature"));
    let fwd: Draft = call(&ctx.svc, "create_draft", json!({"mailboxId": ctx.account, "mode": "forward", "sourceMessageId": src.id})).await;
    assert!(fwd.body_html.contains("Contoso GmbH"));
    for d in [reply, fwd] {
        let _: Value = call(&ctx.svc, "discard_draft", json!({"mailboxId": ctx.account, "draftId": d.id})).await;
    }
    assert!(call_err(&ctx.svc, "save_signatures", json!({"accountId": "nope", "settings": {}})).await.contains("Konto"));
}

#[tokio::test]
async fn contacts_sync_crud_and_lookup() {
    let ctx = setup().await;
    let r: SyncResult = call(&ctx.svc, "sync_contacts", json!({"accountId": ctx.account})).await;
    assert!(r.added >= 10, "{r:?}");
    let all: ContactPage = call(&ctx.svc, "list_contacts", json!({"query": null, "accountId": null})).await;
    assert_eq!(all.total, r.added);
    let names: Vec<String> = all.items.iter().map(|c| c.display_name.to_lowercase()).collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted);
    // Search by company and by name parts.
    let erika: ContactPage = call(&ctx.svc, "list_contacts", json!({"query": "fabrikam"})).await;
    assert_eq!(erika.total, 1);
    let c0 = erika.items[0].clone();
    assert_eq!(c0.source, "o365");
    assert!(!c0.phones.is_empty());

    // Lookup while writing: contacts come first.
    let sugg: Vec<Address> = call(&ctx.svc, "suggest_recipients", json!({"mailboxId": ctx.account, "query": "fabrik"})).await;
    assert_eq!(sugg[0].address, c0.emails[0]);

    // Create → appears in Outlook (fake) and locally.
    let created: Contact = call(
        &ctx.svc,
        "create_contact",
        json!({"accountId": ctx.account, "input": {"givenName": "Paula", "surname": "Neu", "emails": ["paula@example.org"],
            "company": "Beispiel AG", "phones": [{"kind": "mobile", "number": "+49 170 1234567"}]}}),
    )
    .await;
    assert_eq!(created.display_name, "Paula Neu");
    let fake = ctx.fake.as_ref().unwrap();
    let token = fake.issue_token();
    let http = reqwest::Client::new();
    let server: Value = http.get(format!("{}/v1.0/me/contacts", fake.base_url)).bearer_auth(&token)
        .query(&[("$top", "100")]).send().await.unwrap().json().await.unwrap();
    assert!(server["value"].as_array().unwrap().iter().any(|c| c["displayName"] == "Paula Neu"));

    let updated: Contact = call(
        &ctx.svc,
        "update_contact",
        json!({"id": created.id, "input": {"givenName": "Paula", "surname": "Neumann", "emails": ["paula@example.org"], "jobTitle": "CEO"}}),
    )
    .await;
    assert_eq!(updated.display_name, "Paula Neumann");
    assert_eq!(updated.job_title, "CEO");

    // Changes made elsewhere arrive via delta.
    let gid = c0.id.split_once('|').unwrap().1.to_string();
    http.delete(format!("{}/v1.0/me/contacts/{}", fake.base_url, urlencoding::encode(&gid)))
        .bearer_auth(&token).send().await.unwrap().error_for_status().unwrap();
    http.post(format!("{}/v1.0/me/contacts", fake.base_url)).bearer_auth(&token)
        .json(&json!({"displayName": "Von Handy", "emailAddresses": [{"address": "handy@example.org"}]}))
        .send().await.unwrap().error_for_status().unwrap();
    let r: SyncResult = call(&ctx.svc, "sync_contacts", json!({"accountId": ctx.account})).await;
    assert_eq!((r.added, r.removed), (1, 1), "{r:?}");
    assert!(ctx.svc.dispatch("get_contact", json!({"id": c0.id})).await.is_err());

    let _: Value = call(&ctx.svc, "delete_contact", json!({"id": updated.id})).await;
    let left: ContactPage = call(&ctx.svc, "list_contacts", json!({"query": "paula"})).await;
    assert_eq!(left.total, 0);
    assert!(call_err(&ctx.svc, "create_contact", json!({"accountId": ctx.account, "input": {"emails": ["kaputt"]}})).await.contains("Ungültige"));
    assert!(ctx.host.events.lock().unwrap().iter().any(|(e, _)| e == "contacts-changed"));
}

#[tokio::test]
async fn sent_recipients_are_collected() {
    let ctx = setup().await;
    let input = json!({"id": null, "mailboxId": ctx.account, "mode": "new",
        "to": [{"name": "Neuer Kunde", "address": "kunde@neu.example"}], "cc": [], "bcc": [],
        "subject": "Hallo", "bodyHtml": "<p>Hi</p>", "importance": "normal", "requestReadReceipt": false,
        "newAttachments": [], "removedAttachmentIds": []});
    let _: Value = call(&ctx.svc, "send_draft", json!({"draft": input})).await;
    let collected: ContactPage = call(&ctx.svc, "list_contacts", json!({"source": "collected"})).await;
    assert_eq!(collected.total, 1);
    assert_eq!(collected.items[0].display_name, "Neuer Kunde");
    let sugg: Vec<Address> = call(&ctx.svc, "suggest_recipients", json!({"mailboxId": ctx.account, "query": "neuer"})).await;
    assert_eq!(sugg[0].address, "kunde@neu.example");
    // Turning it into a real contact.
    let c: Contact = call(
        &ctx.svc,
        "update_contact",
        json!({"id": collected.items[0].id, "input": {"displayName": "Neuer Kunde", "emails": ["kunde@neu.example"], "company": "Neu GmbH"}}),
    )
    .await;
    assert_eq!(c.source, "local");
    let collected: ContactPage = call(&ctx.svc, "list_contacts", json!({"source": "collected"})).await;
    assert_eq!(collected.total, 0);
}

#[tokio::test]
async fn tasks_are_created_directly_in_place() {
    let ctx = setup().await;
    let inbox = ctx.folder("inbox").await;
    // "Sofort": unread, non-draft, from and to myself, in the inbox.
    let t: MessageSummary = call(
        &ctx.svc,
        "create_task",
        json!({"mailboxId": ctx.account, "subject": "Angebot an Alpha schreiben", "bodyHtml": "<p>bis Freitag</p>", "due": null}),
    )
    .await;
    assert_eq!(t.folder_id, inbox.id);
    assert!(!t.is_read && !t.is_draft);
    assert_eq!(t.from.as_ref().unwrap().address, ME);
    assert_eq!(t.to[0].address, ME);
    assert!(ctx.list(&inbox.id).await.items.iter().any(|m| m.id == t.id));
    // With a due date: straight into "Wiedervorlage", read, due set – also on the server.
    let due = (chrono::Utc::now() + chrono::Duration::days(2)).to_rfc3339();
    let t2: MessageSummary = call(
        &ctx.svc,
        "create_task",
        json!({"mailboxId": ctx.account, "subject": "Steuer vorbereiten", "bodyHtml": "", "due": due}),
    )
    .await;
    let snoozed = ctx.folder("snoozed").await;
    assert_eq!(t2.folder_id, snoozed.id);
    assert!(t2.is_read && !t2.is_draft && t2.due_at.is_some());
    let raw = ctx.fake.as_ref().unwrap().state.message("me", &t2.id).unwrap();
    assert_eq!(raw["isDraft"], false);
    assert_eq!(raw["flag"]["flagStatus"], "flagged");
    // Never shows up in the inbox.
    ctx.sync(&inbox.id).await;
    assert!(!ctx.list(&inbox.id).await.items.iter().any(|m| m.subject == "Steuer vorbereiten"));
    assert!(call_err(&ctx.svc, "create_task", json!({"mailboxId": ctx.account, "subject": " ", "due": null})).await.contains("Betreff"));
}

#[tokio::test]
async fn send_with_reminder_creates_follow_up() {
    let ctx = setup().await;
    let remind = (chrono::Utc::now() + chrono::Duration::days(3)).to_rfc3339();
    let input = json!({"id": null, "mailboxId": ctx.account, "mode": "new",
        "to": [{"name": "Erika Beispiel", "address": "erika@fabrikam.example"}], "cc": [], "bcc": [],
        "subject": "Bitte Rechnung prüfen", "bodyHtml": "<p>Anbei die Rechnung.</p>", "importance": "normal",
        "requestReadReceipt": false, "newAttachments": [], "removedAttachmentIds": [], "remindAt": remind});
    let _: Value = call(&ctx.svc, "send_draft", json!({"draft": input})).await;
    let snoozed = ctx.folder("snoozed").await;
    let items = ctx.list(&snoozed.id).await.items;
    let r = items.iter().find(|m| m.subject == "Nachfassen: Bitte Rechnung prüfen").expect("reminder");
    assert!(r.due_at.is_some());
    assert_eq!(r.to[0].address, "erika@fabrikam.example");
    // Reply goes to the original recipient.
    let reply: Draft = call(&ctx.svc, "create_draft", json!({"mailboxId": ctx.account, "mode": "reply", "sourceMessageId": r.id})).await;
    assert_eq!(reply.to[0].address, "erika@fabrikam.example");
    let d: MessageDetail = call(&ctx.svc, "get_message", json!({"mailboxId": ctx.account, "messageId": r.id})).await;
    assert!(d.body_html.contains("Anbei die Rechnung"));
}

#[tokio::test]
async fn delegated_task_is_sent_and_followed_up() {
    let ctx = setup().await;
    let due = (chrono::Utc::now() + chrono::Duration::days(3)).to_rfc3339();
    let anna = json!({"name": "Anna Muster", "address": "Anna@Fabrikam.example"});
    let t: MessageSummary = call(
        &ctx.svc,
        "create_task",
        json!({"mailboxId": ctx.account, "subject": "Angebot prüfen", "bodyHtml": "<p>bis Freitag</p>", "due": due, "delegate": anna}),
    )
    .await;
    // Copy for me: in "Wiedervorlage", due like a task, marked with the assignee.
    assert_eq!(t.folder_id, ctx.folder("snoozed").await.id);
    assert_eq!(t.subject, "ToDo: Angebot prüfen");
    assert!(t.due_at.is_some());
    assert!(t.categories.contains(&"[D_anna@fabrikam.example]".to_string()), "{:?}", t.categories);
    // The task went out to the assignee.
    let sent = ctx.folder("sentitems").await;
    ctx.sync(&sent.id).await;
    let out = ctx.list(&sent.id).await.items.into_iter().find(|m| m.subject == "ToDo: Angebot prüfen").expect("sent");
    assert_eq!(out.to[0].address, "Anna@Fabrikam.example");
    assert!(t.categories.iter().any(|c| c.starts_with("[DT_")), "{:?}", t.categories);
    // Anna answers in the thread: the reply is recognised as answer to the delegation.
    let fake = ctx.fake.as_ref().unwrap();
    let sent_raw = fake.state.sent_messages("me").into_iter().find(|m| m["subject"] == "ToDo: Angebot prüfen").unwrap();
    fake.state.inject_incoming("me", "inbox", json!({
        "subject": "AW: ToDo: Angebot prüfen", "conversationId": sent_raw["conversationId"], "isRead": false,
        "from": {"emailAddress": {"name": "Anna Muster", "address": "anna@fabrikam.example"}},
    }));
    let inbox = ctx.folder("inbox").await;
    ctx.sync(&inbox.id).await;
    let reply = ctx.list(&inbox.id).await.items.into_iter().find(|m| m.subject == "AW: ToDo: Angebot prüfen").unwrap();
    let found: Option<MessageSummary> = call(&ctx.svc, "find_delegation", json!({"mailboxId": ctx.account, "messageId": reply.id})).await;
    assert_eq!(found.map(|m| m.id), Some(t.id.clone()), "not auto-resolved, but found");
    let other = ctx.list(&inbox.id).await.items.into_iter().find(|m| m.id != reply.id).unwrap();
    let none: Option<MessageSummary> = call(&ctx.svc, "find_delegation", json!({"mailboxId": ctx.account, "messageId": other.id})).await;
    assert!(none.is_none());
    // Without prefix and without due time: copy in the inbox.
    let t2: MessageSummary = call(
        &ctx.svc,
        "create_task",
        json!({"mailboxId": ctx.account, "subject": "Reisekosten", "due": null, "delegate": {"name": "", "address": "bob@fabrikam.example"}, "todoPrefix": false}),
    )
    .await;
    assert_eq!(t2.subject, "Reisekosten");
    let list: Vec<MessageSummary> = call(&ctx.svc, "list_delegations", json!({})).await;
    assert_eq!(list.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), [t.id.as_str(), t2.id.as_str()]);
    // Archived = done: leaves the list.
    let _: Value = call(&ctx.svc, "archive_messages", json!({"mailboxId": ctx.account, "ids": [t2.id]})).await;
    let list: Vec<MessageSummary> = call(&ctx.svc, "list_delegations", json!({})).await;
    assert_eq!(list.len(), 1);
    let bad = json!({"mailboxId": ctx.account, "subject": "x", "due": null, "delegate": {"name": "", "address": "kein-mail"}});
    assert!(call_err(&ctx.svc, "create_task", bad).await.contains("Ungültige Adresse"));
    let rec = json!({"mailboxId": ctx.account, "subject": "x", "due": null, "recurrence": "weekly", "delegate": anna});
    assert!(call_err(&ctx.svc, "create_task", rec).await.contains("wiederholt"));
}

#[tokio::test]
async fn inbox_rules_snooze_self_mails_before_display() {
    let ctx = setup().await;
    let inbox = ctx.folder("inbox").await;
    let fake = ctx.fake.as_ref().unwrap();
    let token = fake.issue_token();
    let http = reqwest::Client::new();
    for subject in ["Angebot prüfen #morgen", "Normale Mail an mich"] {
        http.post(format!("{}/v1.0/me/sendMail", fake.base_url))
            .bearer_auth(&token)
            .json(&json!({"message": {"subject": subject, "body": {"contentType": "text", "content": "x"},
                "toRecipients": [{"emailAddress": {"address": ME}}]}, "saveToSentItems": false}))
            .send().await.unwrap().error_for_status().unwrap();
    }
    ctx.sync(&inbox.id).await;
    let in_inbox = ctx.list(&inbox.id).await.items;
    assert!(!in_inbox.iter().any(|m| m.subject.contains("#morgen")), "rule must move it before display");
    assert!(in_inbox.iter().any(|m| m.subject == "Normale Mail an mich"));
    let snoozed = ctx.folder("snoozed").await;
    let s = ctx.list(&snoozed.id).await.items.into_iter().find(|m| m.subject.contains("#morgen")).unwrap();
    let due = chrono::DateTime::parse_from_rfc3339(s.due_at.as_deref().unwrap()).unwrap().with_timezone(&chrono::Local);
    assert_eq!(due.date_naive(), chrono::Local::now().date_naive() + chrono::Duration::days(1));
    let events = ctx.host.events.lock().unwrap();
    assert!(events.iter().any(|(e, p)| e == "rules-applied" && p["count"] == 1));
    // The moved mail does not trigger a new-mail notification.
    assert!(!events.iter().any(|(e, p)| e == "new-mail" && p["subject"].as_str().unwrap_or("").contains("#morgen")));
}

#[tokio::test]
async fn kanban_lanes_as_hidden_categories() {
    let ctx = setup().await;
    let bad = call_err(&ctx.svc, "save_lanes", json!({"accountId": ctx.account, "lanes": ["To[Do]"]})).await;
    assert!(bad.contains("Ungültiger Lane-Name"));
    let lanes: Vec<String> = call(&ctx.svc, "save_lanes", json!({"accountId": ctx.account, "lanes": ["ToDo", "Warten auf", "Review"]})).await;
    assert_eq!(lanes, ["ToDo", "Warten auf", "Review"]);
    let cats: Vec<Category> = call(&ctx.svc, "list_categories", json!({"accountId": ctx.account})).await;
    assert!(cats.iter().any(|c| c.display_name == "[K_ToDo]"));
    assert!(cats.iter().any(|c| c.display_name == "[K_Warten auf]"));

    let inbox = ctx.folder("inbox").await;
    let m = ctx.list(&inbox.id).await.items.into_iter().find(|m| !m.categories.is_empty()).unwrap();
    let other = m.categories.clone();
    let _: Value = call(&ctx.svc, "set_lane", json!({"mailboxId": ctx.account, "ids": [m.id], "lane": "ToDo"})).await;
    let _: Value = call(&ctx.svc, "set_lane", json!({"mailboxId": ctx.account, "ids": [m.id], "lane": "Review"})).await;
    let after = ctx.list(&inbox.id).await.items.into_iter().find(|x| x.id == m.id).unwrap();
    let mut expect = other.clone();
    expect.push("[K_Review]".into());
    assert_eq!(after.categories, expect, "other categories kept, only one lane");
    let raw = ctx.fake.as_ref().unwrap().state.message("me", &m.id).unwrap();
    assert!(raw["categories"].as_array().unwrap().iter().any(|c| c == "[K_Review]"));
    assert!(call_err(&ctx.svc, "set_lane", json!({"mailboxId": ctx.account, "ids": [m.id], "lane": "Gibtsnicht"})).await.contains("existiert nicht"));

    // Removing a lane deletes its category and strips it from mails.
    let lanes: Vec<String> = call(&ctx.svc, "save_lanes", json!({"accountId": ctx.account, "lanes": ["ToDo", "Warten auf"]})).await;
    assert_eq!(lanes.len(), 2);
    let after = ctx.list(&inbox.id).await.items.into_iter().find(|x| x.id == m.id).unwrap();
    assert_eq!(after.categories, other);
    let cats: Vec<Category> = call(&ctx.svc, "list_categories", json!({"accountId": ctx.account})).await;
    assert!(!cats.iter().any(|c| c.display_name == "[K_Review]"));
    let _: Value = call(&ctx.svc, "set_lane", json!({"mailboxId": ctx.account, "ids": [m.id], "lane": null})).await;
    let got: Vec<String> = call(&ctx.svc, "get_lanes", json!({"accountId": ctx.account})).await;
    assert_eq!(got, ["ToDo", "Warten auf"]);
    let c: Config = call(&ctx.svc, "get_config", json!({})).await;
    assert_eq!(c.kanban_reading_pane, "bottom");
}

#[tokio::test]
async fn follow_up_resolves_itself_when_reply_arrives() {
    let ctx = setup().await;
    let remind = (chrono::Utc::now() + chrono::Duration::days(3)).to_rfc3339();
    let input = json!({"id": null, "mailboxId": ctx.account, "mode": "new",
        "to": [{"name": "Erika Beispiel", "address": "erika@fabrikam.example"}], "cc": [], "bcc": [],
        "subject": "Angebot freigeben?", "bodyHtml": "<p>Bitte bis Freitag.</p>", "importance": "normal",
        "requestReadReceipt": false, "newAttachments": [], "removedAttachmentIds": [], "remindAt": remind});
    let _: Value = call(&ctx.svc, "send_draft", json!({"draft": input})).await;
    let snoozed = ctx.folder("snoozed").await;
    let reminder = ctx.list(&snoozed.id).await.items.into_iter().find(|m| m.subject == "Nachfassen: Angebot freigeben?").unwrap();
    assert!(reminder.categories.iter().any(|c| c.starts_with("[F_")));

    // Erika answers in the same thread.
    let fake = ctx.fake.as_ref().unwrap();
    let sent = fake.state.sent_messages("me").into_iter().find(|m| m["subject"] == "Angebot freigeben?").unwrap();
    fake.state.inject_incoming("me", "inbox", json!({
        "subject": "AW: Angebot freigeben?", "conversationId": sent["conversationId"], "isRead": false,
        "from": {"emailAddress": {"name": "Erika Beispiel", "address": "erika@fabrikam.example"}},
    }));
    let inbox = ctx.folder("inbox").await;
    ctx.sync(&inbox.id).await;
    assert!(!ctx.list(&snoozed.id).await.items.iter().any(|m| m.id == reminder.id), "reminder must be resolved");
    let archive = ctx.folder("archive").await;
    ctx.sync(&archive.id).await;
    assert!(ctx.list(&archive.id).await.items.iter().any(|m| m.subject == "Nachfassen: Angebot freigeben?"));
    assert!(ctx.list(&inbox.id).await.items.iter().any(|m| m.subject == "AW: Angebot freigeben?"));
    assert!(ctx.host.events.lock().unwrap().iter().any(|(e, p)| e == "followup-resolved" && p["from"] == "Erika Beispiel"));
}

#[tokio::test]
async fn recurring_task_reschedules_on_archive() {
    let ctx = setup().await;
    let due = (chrono::Utc::now() + chrono::Duration::seconds(3)).to_rfc3339();
    let t: MessageSummary = call(
        &ctx.svc,
        "create_task",
        json!({"mailboxId": ctx.account, "subject": "Monatsabschluss", "bodyHtml": "<p>Belege</p>", "due": due, "recurrence": "monthly"}),
    )
    .await;
    assert_eq!(t.recurrence.as_deref(), Some("monthly"));
    assert!(t.categories.contains(&"[#monthly]".to_string()));
    // Due → back in the inbox, keeps its due time (anchor of the rhythm).
    tokio::time::sleep(std::time::Duration::from_secs(4)).await;
    ctx.svc.sync_all(true).await;
    let inbox = ctx.folder("inbox").await;
    let back = ctx.list(&inbox.id).await.items.into_iter().find(|m| m.subject == "Monatsabschluss").unwrap();
    assert!(!back.is_read);
    assert!(back.due_at.is_some());
    // Done → archived, next occurrence one month later waits in Wiedervorlage.
    let _: Value = call(&ctx.svc, "archive_messages", json!({"mailboxId": ctx.account, "ids": [back.id]})).await;
    let snoozed = ctx.folder("snoozed").await;
    let next = ctx.list(&snoozed.id).await.items.into_iter().find(|m| m.subject == "Monatsabschluss").expect("next occurrence");
    assert_eq!(next.recurrence.as_deref(), Some("monthly"));
    let d0 = chrono::DateTime::parse_from_rfc3339(back.due_at.as_deref().unwrap()).unwrap().with_timezone(&chrono::Local);
    let d1 = chrono::DateTime::parse_from_rfc3339(next.due_at.as_deref().unwrap()).unwrap().with_timezone(&chrono::Local);
    assert!(d1 > d0 + chrono::Duration::days(27) && d1 < d0 + chrono::Duration::days(32), "{d0} → {d1}");
    assert_eq!(d1.time().format("%H:%M").to_string(), d0.time().format("%H:%M").to_string());
    // Turning recurrence off.
    let _: Value = call(&ctx.svc, "set_recurrence", json!({"mailboxId": ctx.account, "ids": [next.id], "recurrence": null})).await;
    let n = ctx.list(&snoozed.id).await.items.into_iter().find(|m| m.id == next.id).unwrap();
    assert!(n.recurrence.is_none());
    assert!(ctx.host.events.lock().unwrap().iter().any(|(e, _)| e == "recurrence-scheduled"));
}

#[tokio::test]
async fn today_view_and_focus_time() {
    let ctx = setup().await;
    let _: Vec<String> = call(&ctx.svc, "save_lanes", json!({"accountId": ctx.account, "lanes": ["ToDo", "Warten"]})).await;
    let inbox = ctx.folder("inbox").await;
    let m = ctx.list(&inbox.id).await.items[0].clone();
    let _: Value = call(&ctx.svc, "set_lane", json!({"mailboxId": ctx.account, "ids": [m.id], "lane": "ToDo"})).await;
    let soon = (chrono::Utc::now() + chrono::Duration::minutes(2)).to_rfc3339();
    let later = (chrono::Utc::now() + chrono::Duration::days(3)).to_rfc3339();
    let _: MessageSummary = call(&ctx.svc, "create_task", json!({"mailboxId": ctx.account, "subject": "Gleich fällig", "due": soon})).await;
    let _: MessageSummary = call(&ctx.svc, "create_task", json!({"mailboxId": ctx.account, "subject": "Später", "due": later})).await;
    let today: Value = call(&ctx.svc, "list_today", json!({})).await;
    let due: Vec<String> = today["due"].as_array().unwrap().iter().map(|m| m["subject"].as_str().unwrap().to_string()).collect();
    // "Gleich fällig" is due today unless the test runs right before midnight.
    let tomorrow_starts_in = (chrono::Local::now().date_naive() + chrono::Duration::days(1))
        .and_hms_opt(0, 0, 0).unwrap().and_local_timezone(chrono::Local).unwrap() - chrono::Local::now();
    if tomorrow_starts_in > chrono::Duration::minutes(3) {
        assert!(due.contains(&"Gleich fällig".to_string()), "{due:?}");
    }
    assert!(!due.contains(&"Später".to_string()));
    assert_eq!(today["todo"][0]["lane"], "ToDo");
    assert_eq!(today["todo"][0]["items"][0]["id"], m.id.as_str());

    // Fokuszeit pauses the automatic fetch, manual sync still works.
    let f: Value = call(&ctx.svc, "set_focus", json!({"minutes": 60})).await;
    assert!(f["until"].is_string());
    let g: Value = call(&ctx.svc, "get_focus", json!({})).await;
    assert_eq!(g["until"], f["until"]);
    let _: Value = call(&ctx.svc, "set_focus", json!({"minutes": 0})).await;
    let g: Value = call(&ctx.svc, "get_focus", json!({})).await;
    assert!(g["until"].is_null());
    assert!(ctx.host.events.lock().unwrap().iter().filter(|(e, _)| e == "focus-changed").count() >= 2);
}

#[tokio::test]
async fn shared_mailbox_add_sync_remove() {
    let ctx = setup().await;
    const SHARED: &str = "datenschutz@contoso.example";
    let err = call_err(&ctx.svc, "add_shared_mailbox", json!({"accountId": ctx.account, "address": "niemand@contoso.example"})).await;
    assert!(err.contains("nicht gefunden"), "{err}");

    let mb: Mailbox = call(&ctx.svc, "add_shared_mailbox", json!({"accountId": ctx.account, "address": " Datenschutz@Contoso.example "})).await;
    assert_eq!(mb.kind, "shared");
    assert_eq!(mb.address, SHARED);
    assert_eq!(mb.display_name, "Datenschutz Contoso");
    let accounts: Vec<Account> = call(&ctx.svc, "list_accounts", json!({})).await;
    assert_eq!(accounts[0].mailboxes.len(), 2);
    let inbox = ctx
        .folders(&mb.id)
        .await
        .into_iter()
        .find(|f| f.well_known.as_deref() == Some("inbox"))
        .expect("shared inbox synced");
    assert!(inbox.total_count > 0);

    let err = call_err(&ctx.svc, "add_shared_mailbox", json!({"accountId": ctx.account, "address": SHARED})).await;
    assert!(err.contains("bereits"), "{err}");
    let err = call_err(&ctx.svc, "remove_shared_mailbox", json!({"mailboxId": ctx.account})).await;
    assert!(err.contains("Hauptpostfach"), "{err}");

    let _: Value = call(&ctx.svc, "remove_shared_mailbox", json!({"mailboxId": mb.id})).await;
    let accounts: Vec<Account> = call(&ctx.svc, "list_accounts", json!({})).await;
    assert_eq!(accounts[0].mailboxes.len(), 1);
    assert!(ctx.folders(&mb.id).await.is_empty());
}

// ---- actions under server latency (pending changes) ----

/// Waits until a dispatched action has queued itself (its local change is visible).
async fn settle() {
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
}

#[tokio::test]
async fn read_change_shows_at_once_and_survives_a_sync_while_the_server_is_slow() {
    let ctx = setup().await;
    let inbox = ctx.folder("inbox").await;
    let page = ctx.list(&inbox.id).await;
    let m = page.items.iter().find(|m| !m.is_read).unwrap().clone();
    let fake = ctx.fake.as_ref().unwrap();
    fake.state.set_latency("PATCH", "/messages/", std::time::Duration::from_millis(1500));
    let svc = ctx.svc.clone();
    let args = json!({"mailboxId": ctx.account, "ids": [m.id], "isRead": true});
    let running = tokio::spawn(async move { svc.dispatch("set_read", args).await });
    settle().await;
    // Shown before the server answered…
    let now = ctx.list(&inbox.id).await;
    assert!(now.items.iter().find(|x| x.id == m.id).unwrap().is_read);
    assert_eq!(now.unread, page.unread - 1);
    assert_eq!(ctx.folder("inbox").await.unread_count, inbox.unread_count - 1);
    // …and a sync that still sees the old server state does not bring it back.
    assert!(fake.state.update_message("me", &m.id, json!({"subject": "Geändert"})));
    ctx.sync(&inbox.id).await;
    let synced = ctx.list(&inbox.id).await;
    let row = synced.items.iter().find(|x| x.id == m.id).unwrap();
    assert_eq!(row.subject, "Geändert");
    assert!(row.is_read);
    running.await.unwrap().unwrap();
    assert_eq!(fake.state.message("me", &m.id).unwrap()["isRead"], json!(true));
    assert!(ctx.list(&inbox.id).await.items.iter().find(|x| x.id == m.id).unwrap().is_read);
}

#[tokio::test]
async fn archived_mail_leaves_at_once_and_does_not_come_back_with_a_sync() {
    let ctx = setup().await;
    let inbox = ctx.folder("inbox").await;
    let archive = ctx.folder("archive").await;
    let page = ctx.list(&inbox.id).await;
    let m = page.items[0].clone();
    let fake = ctx.fake.as_ref().unwrap();
    fake.state.set_latency("POST", "/move", std::time::Duration::from_millis(1500));
    let svc = ctx.svc.clone();
    let args = json!({"mailboxId": ctx.account, "ids": [m.id]});
    let running = tokio::spawn(async move { svc.dispatch("archive_messages", args).await });
    settle().await;
    let now = ctx.list(&inbox.id).await;
    assert!(!now.items.iter().any(|x| x.id == m.id));
    assert_eq!(now.total, page.total - 1);
    assert_eq!(ctx.folder("inbox").await.total_count, inbox.total_count - 1);
    // The server still has it in the inbox and reports a change: it stays out.
    assert!(fake.state.update_message("me", &m.id, json!({"subject": "Geändert"})));
    ctx.sync(&inbox.id).await;
    assert!(!ctx.list(&inbox.id).await.items.iter().any(|x| x.id == m.id));
    running.await.unwrap().unwrap();
    assert!(!ctx.list(&inbox.id).await.items.iter().any(|x| x.id == m.id));
    assert!(ctx.list(&archive.id).await.items.iter().any(|x| x.subject == "Geändert"));
}

#[tokio::test]
async fn failed_actions_are_undone() {
    let ctx = setup().await;
    let inbox = ctx.folder("inbox").await;
    let page = ctx.list(&inbox.id).await;
    let m = page.items.iter().find(|m| !m.is_read).unwrap().clone();
    let fake = ctx.fake.as_ref().unwrap();

    fake.state.inject_fault(graph_fake::Fault::status(403).on_path("/messages/"));
    call_err(&ctx.svc, "set_read", json!({"mailboxId": ctx.account, "ids": [m.id], "isRead": true})).await;
    let after = ctx.list(&inbox.id).await;
    assert!(!after.items.iter().find(|x| x.id == m.id).unwrap().is_read);
    assert_eq!(after.unread, page.unread);
    assert_eq!(ctx.folder("inbox").await.unread_count, inbox.unread_count);

    fake.state.inject_fault(graph_fake::Fault::status(403).on_path("/move"));
    call_err(&ctx.svc, "archive_messages", json!({"mailboxId": ctx.account, "ids": [m.id]})).await;
    let after = ctx.list(&inbox.id).await;
    assert!(after.items.iter().any(|x| x.id == m.id));
    assert_eq!(after.total, page.total);
}
