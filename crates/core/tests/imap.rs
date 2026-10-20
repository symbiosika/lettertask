//! IMAP/SMTP accounts end to end: a real Dovecot server, a local SMTP sink,
//! everything through `Service::dispatch` like the UI does.

mod support;

use std::sync::Arc;

use base64::Engine;
use futures::TryStreamExt;
use mailcore::host::NullHost;
use mailcore::model::*;
use mailcore::tokens::MemoryTokenStore;
use mailcore::{Service, ServiceOptions};
use serde_json::{json, Value};
use support::dovecot::{Dovecot, PASSWORD};

const ME: &str = "anna@example.org";

struct Ctx {
    svc: Arc<Service>,
    host: Arc<NullHost>,
    dv: Dovecot,
    sink: graph_fake::smtp::SmtpSink,
    _dir: tempfile::TempDir,
    account: String,
}

fn png() -> Vec<u8> {
    base64::engine::general_purpose::STANDARD
        .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==")
        .unwrap()
}

fn seed_messages() -> Vec<(String, &'static str)> {
    let png_b64 = base64::engine::general_purpose::STANDARD.encode(png());
    vec![
        (
            "From: Bernd Beispiel <bernd@example.com>\r\nTo: Anna <anna@example.org>\r\nCc: Carl <carl@example.com>\r\nSubject: =?UTF-8?Q?Gr=C3=BC=C3=9Fe_aus_M=C3=BCnchen?=\r\nDate: Mon, 12 Oct 2026 09:00:00 +0200\r\nMessage-ID: <m1@example.com>\r\nMIME-Version: 1.0\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<html><body><p>Hallo Anna,</p><p>viele Gr\u{fc}\u{df}e!</p></body></html>\r\n".to_string(),
            "(\\Seen)",
        ),
        (
            "From: Clara Rechnung <clara@shop.example>\r\nTo: anna@example.org\r\nSubject: Rechnung Oktober\r\nDate: Tue, 13 Oct 2026 10:00:00 +0200\r\nMessage-ID: <m2@shop.example>\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"b1\"\r\n\r\n--b1\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nAnbei die Rechnung.\r\n--b1\r\nContent-Type: text/plain; name=\"rechnung.txt\"\r\nContent-Disposition: attachment; filename=\"rechnung.txt\"\r\n\r\nBetrag: 42 EUR\r\n--b1--\r\n".to_string(),
            "()",
        ),
        (
            format!("From: News <news@example.net>\r\nTo: anna@example.org\r\nSubject: Newsletter mit Bild\r\nDate: Wed, 14 Oct 2026 11:00:00 +0200\r\nMessage-ID: <m3@example.net>\r\nMIME-Version: 1.0\r\nContent-Type: multipart/related; boundary=\"r1\"\r\n\r\n--r1\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<html><body><img src=\"cid:logo@news\"><p>Neuigkeiten</p></body></html>\r\n--r1\r\nContent-Type: image/png\r\nContent-ID: <logo@news>\r\nContent-Transfer-Encoding: base64\r\nContent-Disposition: inline\r\n\r\n{png_b64}\r\n--r1--\r\n"),
            "(\\Seen)",
        ),
        (
            "From: Bernd Beispiel <bernd@example.com>\r\nTo: anna@example.org\r\nSubject: Projekt Alpha Status\r\nDate: Thu, 15 Oct 2026 12:00:00 +0200\r\nMessage-ID: <m4@example.com>\r\nX-Priority: 1\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nDer Status ist gruen.\r\n".to_string(),
            "()",
        ),
    ]
}

fn imap_settings(port: u16) -> mailcore::imap::ServerSettings {
    mailcore::imap::ServerSettings {
        host: "127.0.0.1".into(),
        port,
        security: "none".into(),
        username: ME.into(),
    }
}

async fn raw_conn(port: u16) -> mailcore::imap::conn::Conn {
    mailcore::imap::conn::connect(&imap_settings(port), PASSWORD).await.unwrap()
}

async fn seed(port: u16) {
    let mut c = raw_conn(port).await;
    for (raw, flags) in seed_messages() {
        c.session.append("INBOX", Some(flags), None, raw.as_bytes()).await.unwrap();
    }
    c.session.logout().await.unwrap();
}

/// Subject → flags of all messages in `folder`, straight from the server.
async fn server_flags(port: u16, folder: &str) -> Vec<(String, Vec<String>)> {
    let mut c = raw_conn(port).await;
    c.session.select(folder).await.unwrap();
    let fetches: Vec<_> = c
        .session
        .fetch("1:*", "(UID FLAGS BODY.PEEK[HEADER])")
        .await
        .map(|s| s.try_collect::<Vec<_>>())
        .unwrap()
        .await
        .unwrap_or_default();
    let mut out = Vec::new();
    for f in &fetches {
        let flags = mailcore::imap::mime::flag_strings(f.flags());
        let subject = mail_parser::MessageParser::default()
            .parse(f.header().unwrap_or_default())
            .and_then(|m| m.subject().map(str::to_string))
            .unwrap_or_default();
        out.push((subject, flags));
    }
    drop(fetches);
    let _ = c.session.logout().await;
    out
}

async fn server_raw(port: u16, folder: &str, subject: &str) -> Option<String> {
    let mut c = raw_conn(port).await;
    c.session.select(folder).await.unwrap();
    let fetches: Vec<_> = c
        .session
        .fetch("1:*", "(BODY.PEEK[])")
        .await
        .map(|s| s.try_collect::<Vec<_>>())
        .ok()?
        .await
        .unwrap_or_default();
    let found = fetches
        .iter()
        .filter_map(|f| f.body().map(|b| String::from_utf8_lossy(b).into_owned()))
        .find(|raw| raw.contains(subject));
    drop(fetches);
    let _ = c.session.logout().await;
    found
}

fn input(dv_port: u16, smtp_port: u16, password: &str) -> Value {
    json!({
        "email": ME, "displayName": "Anna Beispiel",
        "imap": {"host": "127.0.0.1", "port": dv_port, "security": "none", "username": ME},
        "smtp": {"host": "127.0.0.1", "port": smtp_port, "security": "none", "username": ME},
        "password": password, "smtpPassword": null
    })
}

async fn setup() -> Option<Ctx> {
    let dv = Dovecot::start()?;
    seed(dv.port).await;
    let sink = graph_fake::smtp::spawn_sink(PASSWORD).await;
    let dir = tempfile::tempdir().unwrap();
    let host = Arc::new(NullHost::default());
    let svc = Service::new(
        ServiceOptions {
            data_dir: dir.path().to_path_buf(),
            graph_base: "http://127.0.0.1:9/v1.0".into(),
            authority: "http://127.0.0.1:9".into(),
            demo: false,
            token_store: Arc::new(MemoryTokenStore::default()),
        },
        host.clone(),
    )
    .unwrap();
    let acc: Account = call(&svc, "add_imap_account", json!({"input": input(dv.port, sink.port, PASSWORD)})).await;
    Some(Ctx {
        svc,
        host,
        dv,
        sink,
        _dir: dir,
        account: acc.id,
    })
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
    async fn folders(&self) -> Vec<Folder> {
        call(&self.svc, "list_folders", json!({"mailboxId": self.account})).await
    }
    async fn folder(&self, wk: &str) -> Folder {
        self.folders()
            .await
            .into_iter()
            .find(|f| f.well_known.as_deref() == Some(wk))
            .unwrap_or_else(|| panic!("folder {wk} missing"))
    }
    async fn list(&self, folder: &str) -> MessagePage {
        call(
            &self.svc,
            "list_messages",
            json!({"mailboxId": self.account, "folderId": folder, "sortBy": "received", "sortDesc": true, "limit": 500}),
        )
        .await
    }
    async fn sync(&self, folder: &str) -> SyncResult {
        call(&self.svc, "sync_folder", json!({"mailboxId": self.account, "folderId": folder})).await
    }
    async fn by_subject(&self, folder: &str, subject: &str) -> MessageSummary {
        self.list(folder)
            .await
            .items
            .into_iter()
            .find(|m| m.subject == subject)
            .unwrap_or_else(|| panic!("{subject} not in {folder}"))
    }
}

#[tokio::test]
async fn account_setup_folders_and_sync() {
    let Some(ctx) = setup().await else { return };
    // Wrong password is rejected with a German message, nothing is stored.
    let svc2 = &ctx.svc;
    let mut bad = input(ctx.dv.port, ctx.sink.port, "falsch");
    bad["email"] = json!("bob@example.org");
    bad["imap"]["username"] = json!("bob@example.org");
    let err = call_err(svc2, "add_imap_account", json!({"input": bad})).await;
    assert!(err.contains("Passwort falsch"), "{err}");

    let accounts: Vec<Account> = call(&ctx.svc, "list_accounts", json!({})).await;
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].kind, "imap");
    assert_eq!(accounts[0].id, format!("imap:{ME}"));
    assert_eq!(accounts[0].display_name, "Anna Beispiel");

    let folders = ctx.folders().await;
    let wk: Vec<_> = folders.iter().filter_map(|f| f.well_known.clone()).collect();
    for w in ["inbox", "drafts", "sentitems", "deleteditems", "archive", "junkemail"] {
        assert!(wk.iter().any(|x| x == w), "{w} missing in {wk:?}");
    }
    let inbox = ctx.folder("inbox").await;
    assert_eq!(inbox.display_name, "Posteingang");
    assert_eq!(inbox.total_count, 4);
    assert_eq!(inbox.unread_count, 2);

    let page = ctx.list(&inbox.id).await;
    assert_eq!(page.total, 4);
    assert_eq!(page.items[0].subject, "Projekt Alpha Status");
    assert_eq!(page.items[0].importance, "high");
    assert!(page.items.iter().any(|m| m.subject == "Grüße aus München" && m.is_read));
    let invoice = page.items.iter().find(|m| m.subject == "Rechnung Oktober").unwrap();
    assert!(invoice.has_attachments);
    assert_eq!(invoice.from.as_ref().unwrap().name, "Clara Rechnung");
    assert!(invoice.preview.contains("Anbei die Rechnung"), "{}", invoice.preview);
    assert!(!page.can_load_older);

    // Nothing changed → nothing reported.
    let r = ctx.sync(&inbox.id).await;
    assert_eq!((r.added, r.updated, r.removed), (0, 0, 0));

    // Calendar features are simply empty for IMAP accounts.
    let cals: Vec<Calendar> = call(&ctx.svc, "list_calendars", json!({"accountId": ctx.account})).await;
    assert!(cals.is_empty());
    let get: mailcore::imap::ImapAccountInput = call(&ctx.svc, "get_imap_account", json!({"accountId": ctx.account})).await;
    assert_eq!(get.password, "");
    assert_eq!(get.imap.port, ctx.dv.port);
}

#[tokio::test]
async fn flags_roundtrip_and_server_changes() {
    let Some(ctx) = setup().await else { return };
    let inbox = ctx.folder("inbox").await;
    let alpha = ctx.by_subject(&inbox.id, "Projekt Alpha Status").await;
    let ids = json!([alpha.id]);
    let _: Value = call(&ctx.svc, "set_read", json!({"mailboxId": ctx.account, "ids": ids, "isRead": true})).await;
    let _: Value = call(&ctx.svc, "set_flag", json!({"mailboxId": ctx.account, "ids": ids, "flag": "flagged"})).await;
    let flags = server_flags(ctx.dv.port, "INBOX").await;
    let f = &flags.iter().find(|(s, _)| s == "Projekt Alpha Status").unwrap().1;
    assert!(f.contains(&"\\Seen".to_string()) && f.contains(&"\\Flagged".to_string()), "{f:?}");
    let _: Value = call(&ctx.svc, "set_flag", json!({"mailboxId": ctx.account, "ids": ids, "flag": "complete"})).await;
    let flags = server_flags(ctx.dv.port, "INBOX").await;
    let f = &flags.iter().find(|(s, _)| s == "Projekt Alpha Status").unwrap().1;
    assert!(f.contains(&"$Completed".to_string()) && !f.contains(&"\\Flagged".to_string()), "{f:?}");
    assert_eq!(ctx.by_subject(&inbox.id, "Projekt Alpha Status").await.flag, "complete");
    assert_eq!(ctx.folder("inbox").await.unread_count, 1);

    // Another client marks a mail read and deletes one.
    let mut c = raw_conn(ctx.dv.port).await;
    c.session.select("INBOX").await.unwrap();
    let uids: Vec<u32> = c.session.uid_search("SUBJECT \"Rechnung\"").await.unwrap().into_iter().collect();
    let _: Vec<_> = c.session.uid_store(uids[0].to_string(), "+FLAGS (\\Seen)").await.unwrap().try_collect().await.unwrap();
    let uids: Vec<u32> = c.session.uid_search("SUBJECT \"Newsletter\"").await.unwrap().into_iter().collect();
    let _: Vec<_> = c.session.uid_store(uids[0].to_string(), "+FLAGS (\\Deleted)").await.unwrap().try_collect().await.unwrap();
    let _: Vec<_> = c.session.expunge().await.unwrap().try_collect().await.unwrap();
    // …and a new one arrives.
    c.session
        .append("INBOX", None, None, b"From: Dora <dora@example.com>\r\nTo: anna@example.org\r\nSubject: Neu per IMAP\r\nMessage-ID: <m9@example.com>\r\n\r\nHallo\r\n")
        .await
        .unwrap();
    c.session.logout().await.unwrap();

    let r = ctx.sync(&inbox.id).await;
    assert_eq!((r.added, r.updated, r.removed), (1, 1, 1), "{r:?}");
    let page = ctx.list(&inbox.id).await;
    assert!(!page.items.iter().any(|m| m.subject == "Newsletter mit Bild"));
    assert!(page.items.iter().find(|m| m.subject == "Rechnung Oktober").unwrap().is_read);
    let new = page.items.iter().find(|m| m.subject == "Neu per IMAP").unwrap();
    assert!(!new.is_read);
    assert!(ctx.host.events.lock().unwrap().iter().any(|(e, p)| e == "new-mail" && p["subject"] == "Neu per IMAP"));
}

#[tokio::test]
async fn categories_are_keywords_with_local_master_list() {
    let Some(ctx) = setup().await else { return };
    let cat: Category = call(
        &ctx.svc,
        "create_category",
        json!({"accountId": ctx.account, "displayName": "Business & Pitch", "color": "preset7"}),
    )
    .await;
    let dup = call_err(&ctx.svc, "create_category", json!({"accountId": ctx.account, "displayName": "business & pitch", "color": "preset1"})).await;
    assert!(dup.contains("existiert bereits"));
    let inbox = ctx.folder("inbox").await;
    let m = ctx.by_subject(&inbox.id, "Rechnung Oktober").await;
    let _: Value = call(
        &ctx.svc,
        "set_categories",
        json!({"mailboxId": ctx.account, "ids": [m.id], "categories": ["Business & Pitch", "Zahlung"]}),
    )
    .await;
    let flags = server_flags(ctx.dv.port, "INBOX").await;
    let f = &flags.iter().find(|(s, _)| s == "Rechnung Oktober").unwrap().1;
    assert!(f.contains(&mailcore::imap::category_keyword("Business & Pitch")), "{f:?}");
    assert!(f.contains(&mailcore::imap::category_keyword("Zahlung")), "{f:?}");
    // Unknown names were added to the account's list.
    let cats: Vec<Category> = call(&ctx.svc, "list_categories", json!({"accountId": ctx.account})).await;
    assert!(cats.iter().any(|c| c.display_name == "Zahlung"));
    // A fresh sync reads them back from the server.
    let _: Value = ctx.svc.dispatch("sync_folder", json!({"mailboxId": ctx.account, "folderId": inbox.id})).await.unwrap();
    assert_eq!(ctx.by_subject(&inbox.id, "Rechnung Oktober").await.categories.len(), 2);
    let _: Value = call(&ctx.svc, "set_categories", json!({"mailboxId": ctx.account, "ids": [m.id], "categories": ["Zahlung"]})).await;
    let flags = server_flags(ctx.dv.port, "INBOX").await;
    let f = &flags.iter().find(|(s, _)| s == "Rechnung Oktober").unwrap().1;
    assert!(!f.contains(&mailcore::imap::category_keyword("Business & Pitch")), "{f:?}");
    let u: Category = call(&ctx.svc, "update_category", json!({"accountId": ctx.account, "id": cat.id, "color": "preset3"})).await;
    assert_eq!(u.color, "preset3");
    let _: Value = call(&ctx.svc, "delete_category", json!({"accountId": ctx.account, "id": cat.id})).await;
    let cats: Vec<Category> = call(&ctx.svc, "list_categories", json!({"accountId": ctx.account})).await;
    assert!(!cats.iter().any(|c| c.id == cat.id));
}

#[tokio::test]
async fn folders_move_copy_delete_archive() {
    let Some(ctx) = setup().await else { return };
    let inbox = ctx.folder("inbox").await;
    let proj: Folder = call(&ctx.svc, "create_folder", json!({"mailboxId": ctx.account, "parentId": null, "name": "Projekte"})).await;
    let sub: Folder = call(&ctx.svc, "create_folder", json!({"mailboxId": ctx.account, "parentId": proj.id, "name": "Kunde Ä"})).await;
    assert_eq!(sub.parent_id.as_deref(), Some(proj.id.as_str()));
    assert_eq!(sub.display_name, "Kunde Ä");

    let a = ctx.by_subject(&inbox.id, "Projekt Alpha Status").await;
    let b = ctx.by_subject(&inbox.id, "Grüße aus München").await;
    let _: Value = call(&ctx.svc, "move_messages", json!({"mailboxId": ctx.account, "ids": [a.id], "destFolderId": sub.id})).await;
    let _: Value = call(&ctx.svc, "copy_messages", json!({"mailboxId": ctx.account, "ids": [b.id], "destFolderId": sub.id})).await;
    let in_sub = ctx.list(&sub.id).await;
    assert_eq!(in_sub.total, 2);
    assert!(!ctx.list(&inbox.id).await.items.iter().any(|m| m.subject == "Projekt Alpha Status"));
    assert_eq!(server_flags(ctx.dv.port, &sub.id).await.len(), 2);

    // Delete → Trash, delete in Trash → gone.
    let moved = ctx.by_subject(&sub.id, "Projekt Alpha Status").await;
    let _: Value = call(&ctx.svc, "delete_messages", json!({"mailboxId": ctx.account, "ids": [moved.id]})).await;
    let trash = ctx.folder("deleteditems").await;
    let in_trash = ctx.by_subject(&trash.id, "Projekt Alpha Status").await;
    let _: Value = call(&ctx.svc, "delete_messages", json!({"mailboxId": ctx.account, "ids": [in_trash.id]})).await;
    assert!(server_flags(ctx.dv.port, &trash.id).await.is_empty());
    assert_eq!(ctx.list(&trash.id).await.total, 0);

    let _: Value = call(&ctx.svc, "archive_messages", json!({"mailboxId": ctx.account, "ids": [b.id]})).await;
    let archive = ctx.folder("archive").await;
    ctx.by_subject(&archive.id, "Grüße aus München").await;

    let renamed: Folder = call(&ctx.svc, "rename_folder", json!({"mailboxId": ctx.account, "folderId": sub.id, "name": "Kunde B"})).await;
    assert_eq!(renamed.display_name, "Kunde B");
    assert!(call_err(&ctx.svc, "delete_folder", json!({"mailboxId": ctx.account, "folderId": inbox.id})).await.contains("Standardordner"));
    let _: Value = call(&ctx.svc, "delete_folder", json!({"mailboxId": ctx.account, "folderId": renamed.id})).await;
    assert!(!ctx.folders().await.iter().any(|f| f.id == renamed.id));

    let _: Value = call(&ctx.svc, "mark_folder_read", json!({"mailboxId": ctx.account, "folderId": inbox.id})).await;
    assert_eq!(ctx.folder("inbox").await.unread_count, 0);
    assert!(server_flags(ctx.dv.port, "INBOX").await.iter().all(|(_, f)| f.contains(&"\\Seen".to_string())));
    let _: Value = call(&ctx.svc, "empty_folder", json!({"mailboxId": ctx.account, "folderId": archive.id})).await;
    assert!(server_flags(ctx.dv.port, &archive.id).await.is_empty());
}

#[tokio::test]
async fn snooze_returns_mail_to_inbox() {
    let Some(ctx) = setup().await else { return };
    let inbox = ctx.folder("inbox").await;
    let a = ctx.by_subject(&inbox.id, "Grüße aus München").await;
    let b = ctx.by_subject(&inbox.id, "Rechnung Oktober").await;
    let soon = (chrono::Utc::now() + chrono::Duration::seconds(3)).to_rfc3339();
    let later = (chrono::Utc::now() + chrono::Duration::days(2)).to_rfc3339();
    let _: Value = call(&ctx.svc, "snooze_messages", json!({"mailboxId": ctx.account, "ids": [a.id], "until": soon})).await;
    let _: Value = call(&ctx.svc, "snooze_messages", json!({"mailboxId": ctx.account, "ids": [b.id], "until": later})).await;
    let snoozed = ctx.folder("snoozed").await;
    assert_eq!(snoozed.display_name, "Wiedervorlage");
    let page = ctx.list(&snoozed.id).await;
    assert_eq!(page.total, 2);
    assert!(page.items.iter().all(|m| m.due_at.is_some()), "{:?}", page.items);
    tokio::time::sleep(std::time::Duration::from_secs(4)).await;
    ctx.svc.sync_all(true).await;
    let back = ctx.by_subject(&inbox.id, "Grüße aus München").await;
    assert!(!back.is_read);
    assert!(back.due_at.is_none());
    assert_eq!(ctx.list(&snoozed.id).await.total, 1);
    let still = ctx.by_subject(&snoozed.id, "Rechnung Oktober").await;
    let _: Value = call(&ctx.svc, "unsnooze_messages", json!({"mailboxId": ctx.account, "ids": [still.id]})).await;
    assert_eq!(ctx.list(&snoozed.id).await.total, 0);
    ctx.by_subject(&inbox.id, "Rechnung Oktober").await;
}

#[tokio::test]
async fn search_fields() {
    let Some(ctx) = setup().await else { return };
    let inbox = ctx.folder("inbox").await;
    let s = async |text: &str, field: &str, att: bool, folder: Option<&str>| -> Vec<String> {
        let r: Vec<MessageSummary> = call(
            &ctx.svc,
            "search_messages",
            json!({"mailboxId": ctx.account, "text": text, "folderId": folder, "field": field, "hasAttachments": att}),
        )
        .await;
        r.into_iter().map(|m| m.subject).collect()
    };
    assert_eq!(s("Clara", "from", false, None).await, ["Rechnung Oktober"]);
    assert!(s("Clara", "subject", false, None).await.is_empty());
    assert_eq!(s("Alpha", "subject", false, Some(&inbox.id)).await, ["Projekt Alpha Status"]);
    let all = s("Bernd", "all", false, None).await;
    assert_eq!(all.len(), 2);
    assert_eq!(s("Rechnung", "all", true, None).await, ["Rechnung Oktober"]);
    assert!(s("Bernd", "from", true, None).await.is_empty());
}

#[tokio::test]
async fn message_detail_attachments_and_inline_images() {
    let Some(ctx) = setup().await else { return };
    let inbox = ctx.folder("inbox").await;
    let inv = ctx.by_subject(&inbox.id, "Rechnung Oktober").await;
    let d: MessageDetail = call(&ctx.svc, "get_message", json!({"mailboxId": ctx.account, "messageId": inv.id})).await;
    assert_eq!(d.attachments.len(), 1);
    assert_eq!(d.attachments[0].name, "rechnung.txt");
    assert!(d.body_html.contains("Anbei die Rechnung"));
    let path: Option<String> = call(
        &ctx.svc,
        "download_attachment",
        json!({"mailboxId": ctx.account, "messageId": inv.id, "attachmentId": d.attachments[0].id, "action": "save"}),
    )
    .await;
    assert_eq!(std::fs::read_to_string(path.unwrap()).unwrap().trim(), "Betrag: 42 EUR");
    let news = ctx.by_subject(&inbox.id, "Newsletter mit Bild").await;
    let d: MessageDetail = call(&ctx.svc, "get_message", json!({"mailboxId": ctx.account, "messageId": news.id})).await;
    assert!(d.body_html.contains("data:image/png;base64,"));
    assert!(d.attachments.is_empty());
    let g = ctx.by_subject(&inbox.id, "Grüße aus München").await;
    let d: MessageDetail = call(&ctx.svc, "get_message", json!({"mailboxId": ctx.account, "messageId": g.id})).await;
    assert_eq!(d.cc[0].address, "carl@example.com");
    assert_eq!(d.internet_message_id.as_deref(), Some("<m1@example.com>"));
    let src: String = call(&ctx.svc, "get_message_source", json!({"mailboxId": ctx.account, "messageId": g.id})).await;
    assert!(src.contains("Message-ID: <m1@example.com>"));
}

#[tokio::test]
async fn drafts_replies_and_sending() {
    let Some(ctx) = setup().await else { return };
    let inbox = ctx.folder("inbox").await;
    let drafts = ctx.folder("drafts").await;
    let g = ctx.by_subject(&inbox.id, "Grüße aus München").await;

    // Reply all: AW:, sender + other recipients (not me), quoted body, In-Reply-To.
    let reply: Draft = call(&ctx.svc, "create_draft", json!({"mailboxId": ctx.account, "mode": "replyAll", "sourceMessageId": g.id})).await;
    assert_eq!(reply.subject, "AW: Grüße aus München");
    assert_eq!(reply.to[0].address, "bernd@example.com");
    assert!(!reply.to.iter().chain(&reply.cc).any(|a| a.address == ME));
    assert!(reply.cc.iter().any(|a| a.address == "carl@example.com"));
    assert!(reply.body_html.contains("viele Grüße"));
    assert!(reply.body_html.contains("<b>Von:</b>"));
    let raw = server_raw(ctx.dv.port, &drafts.id, "AW: =?utf-8?").await.or(server_raw(ctx.dv.port, &drafts.id, "In-Reply-To").await).unwrap();
    assert!(raw.contains("In-Reply-To: <m1@example.com>"), "{raw}");
    let _: Value = call(&ctx.svc, "discard_draft", json!({"mailboxId": ctx.account, "draftId": reply.id})).await;
    assert!(server_flags(ctx.dv.port, &drafts.id).await.is_empty());

    // Forward keeps the attachment.
    let inv = ctx.by_subject(&inbox.id, "Rechnung Oktober").await;
    let fwd: Draft = call(&ctx.svc, "create_draft", json!({"mailboxId": ctx.account, "mode": "forward", "sourceMessageId": inv.id})).await;
    assert_eq!(fwd.subject, "WG: Rechnung Oktober");
    assert!(fwd.to.is_empty());
    assert_eq!(fwd.attachments.len(), 1);

    // Save twice: one draft in Drafts with the latest subject.
    let b64 = |s: &[u8]| base64::engine::general_purpose::STANDARD.encode(s);
    let mut d = json!({"id": fwd.id, "mailboxId": ctx.account, "mode": "forward",
        "to": [{"name": "Erik", "address": "erik@example.com"}], "cc": [], "bcc": [{"name": "", "address": "geheim@example.com"}],
        "subject": "WG: Rechnung Oktober (1)", "bodyHtml": format!("<p>Siehe unten</p>{}", fwd.body_html),
        "importance": "high", "requestReadReceipt": false,
        "newAttachments": [{"name": "notiz.txt", "contentType": "text/plain", "contentBase64": b64(b"Notiz")}],
        "removedAttachmentIds": []});
    let saved: Draft = call(&ctx.svc, "save_draft", json!({"draft": d})).await;
    assert_eq!(saved.attachments.len(), 2);
    d["id"] = json!(saved.id);
    d["subject"] = json!("WG: Rechnung Oktober (2)");
    d["newAttachments"] = json!([]);
    let saved2: Draft = call(&ctx.svc, "save_draft", json!({"draft": d})).await;
    let on_server = server_flags(ctx.dv.port, &drafts.id).await;
    assert_eq!(on_server.len(), 1, "{on_server:?}");
    assert_eq!(on_server[0].0, "WG: Rechnung Oktober (2)");
    assert!(on_server[0].1.contains(&"\\Draft".to_string()));
    assert_eq!(saved2.attachments.len(), 2);
    assert_eq!(saved2.bcc[0].address, "geheim@example.com");
    assert_eq!(saved2.importance, "high");

    // Send: SMTP sink gets it (with Bcc only in the envelope), copy in Sent, draft gone.
    let notiz = saved2.attachments.iter().find(|a| a.name == "notiz.txt").unwrap().id.clone();
    d["id"] = json!(saved2.id);
    d["removedAttachmentIds"] = json!([notiz]);
    let _: Value = call(&ctx.svc, "send_draft", json!({"draft": d})).await;
    let msgs = ctx.sink.messages();
    assert_eq!(msgs.len(), 1);
    let m = &msgs[0];
    assert_eq!(m.from, ME);
    assert!(m.rcpts.contains(&"erik@example.com".to_string()) && m.rcpts.contains(&"geheim@example.com".to_string()));
    assert!(m.raw.contains("rechnung.txt"));
    assert!(!m.raw.contains("notiz.txt"));
    assert!(!m.raw.to_lowercase().contains("bcc:"));
    assert!(server_flags(ctx.dv.port, &drafts.id).await.is_empty());
    let sent = ctx.folder("sentitems").await;
    let in_sent = ctx.by_subject(&sent.id, "WG: Rechnung Oktober (2)").await;
    assert!(in_sent.is_read);
    // Recipients were collected as contacts.
    let contacts: Value = ctx.svc.dispatch("list_contacts", json!({"query": "erik", "accountId": null, "offset": 0, "limit": 10})).await.unwrap();
    assert!(contacts.to_string().contains("erik@example.com"), "{contacts}");

    // New mail without id; validation.
    let new: Draft = call(&ctx.svc, "create_draft", json!({"mailboxId": ctx.account, "mode": "new"})).await;
    assert!(new.id.is_none());
    let err = call_err(&ctx.svc, "send_draft", json!({"draft": {"id": null, "mailboxId": ctx.account, "subject": "x"}})).await;
    assert!(err.contains("Empfänger"));
}

#[tokio::test]
async fn smtp_login_errors_and_connection_test() {
    let Some(ctx) = setup().await else { return };
    let mut i = input(ctx.dv.port, ctx.sink.port, PASSWORD);
    i["smtpPassword"] = json!("falsch");
    let r: Value = call(&ctx.svc, "test_imap_account", json!({"input": i})).await;
    assert_eq!(r["imapOk"], true);
    assert_eq!(r["smtpOk"], false);
    assert!(r["smtpError"].as_str().unwrap().contains("SMTP-Anmeldung"), "{r}");
    let r: Value = call(&ctx.svc, "test_imap_account", json!({"input": input(ctx.dv.port, ctx.sink.port, PASSWORD)})).await;
    assert_eq!((r["imapOk"].as_bool(), r["smtpOk"].as_bool()), (Some(true), Some(true)), "{r}");
    // Update keeps the stored password when none is given.
    let mut upd = input(ctx.dv.port, ctx.sink.port, "");
    upd["displayName"] = json!("Anna B.");
    let acc: Account = call(&ctx.svc, "update_imap_account", json!({"accountId": ctx.account, "input": upd})).await;
    assert_eq!(acc.display_name, "Anna B.");
}

#[tokio::test]
async fn works_offline_from_cache() {
    let Some(mut ctx) = setup().await else { return };
    let inbox = ctx.folder("inbox").await;
    let g = ctx.by_subject(&inbox.id, "Grüße aus München").await;
    let online: MessageDetail = call(&ctx.svc, "get_message", json!({"mailboxId": ctx.account, "messageId": g.id})).await;
    ctx.dv.stop();
    assert_eq!(ctx.list(&inbox.id).await.total, 4);
    let offline: MessageDetail = call(&ctx.svc, "get_message", json!({"mailboxId": ctx.account, "messageId": g.id})).await;
    assert_eq!(offline.body_html, online.body_html);
    ctx.svc.sync_all(false).await;
    let events = ctx.host.events.lock().unwrap();
    let last = events.iter().rev().find(|(e, _)| e == "sync-status").unwrap();
    assert_eq!(last.1["state"], "offline", "{:?}", last.1);
}

#[tokio::test]
async fn remove_account_deletes_credentials() {
    let Some(ctx) = setup().await else { return };
    let _: Value = call(&ctx.svc, "remove_account", json!({"accountId": ctx.account})).await;
    let accounts: Vec<Account> = call(&ctx.svc, "list_accounts", json!({})).await;
    assert!(accounts.is_empty());
    let err = call_err(&ctx.svc, "sync_folders", json!({"mailboxId": ctx.account})).await;
    assert!(!err.is_empty());
}

#[tokio::test]
async fn discover_settings() {
    use axum::routing::get;
    let xml = r#"<clientConfig version="1.1"><emailProvider id="firma.test">
<incomingServer type="imap"><hostname>mail.firma.test</hostname><port>993</port><socketType>SSL</socketType><username>%EMAILADDRESS%</username></incomingServer>
<outgoingServer type="smtp"><hostname>mail.firma.test</hostname><port>465</port><socketType>SSL</socketType><username>%EMAILLOCALPART%</username></outgoingServer>
</emailProvider></clientConfig>"#;
    let app = axum::Router::new().route("/firma.test", get(move || async move { xml }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    // SAFETY: only this test reads the variable.
    unsafe { std::env::set_var(mailcore::imap::discover::ISPDB_ENV, format!("http://127.0.0.1:{port}")) };
    let dir = tempfile::tempdir().unwrap();
    let svc = Service::new(
        ServiceOptions {
            data_dir: dir.path().to_path_buf(),
            graph_base: "http://127.0.0.1:9/v1.0".into(),
            authority: "http://127.0.0.1:9".into(),
            demo: false,
            token_store: Arc::new(MemoryTokenStore::default()),
        },
        Arc::new(NullHost::default()),
    )
    .unwrap();
    let d: Value = call(&svc, "discover_imap", json!({"email": "max@firma.test"})).await;
    assert_eq!(d["source"], "ispdb");
    assert_eq!(d["imap"]["host"], "mail.firma.test");
    assert_eq!(d["smtp"]["port"], 465);
    assert_eq!(d["smtp"]["username"], "max");
    let g: Value = call(&svc, "discover_imap", json!({"email": "max@unbekannt.invalid"})).await;
    assert_eq!(g["source"], "guess");
    assert_eq!(g["imap"]["host"], "imap.unbekannt.invalid");
    assert_eq!(g["smtp"]["security"], "starttls");
}
