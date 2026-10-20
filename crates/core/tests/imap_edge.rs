//! IMAP/SMTP edge cases against a real Dovecot: hostile input, other clients
//! changing the mailbox behind our back, odd folder names and server layouts,
//! broken connections. Everything goes through `Service::dispatch`.

mod support;

use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine;
use futures::TryStreamExt;
use mailcore::host::NullHost;
use mailcore::imap::conn::Conn;
use mailcore::model::*;
use mailcore::tokens::MemoryTokenStore;
use mailcore::{Service, ServiceOptions};
use serde_json::{json, Value};
use support::dovecot::{Dovecot, Options, PASSWORD};

const ME: &str = "anna@example.org";

struct Ctx {
    svc: Arc<Service>,
    dv: Dovecot,
    sink: graph_fake::smtp::SmtpSink,
    _dir: tempfile::TempDir,
    account: String,
}

fn settings(port: u16) -> mailcore::imap::ServerSettings {
    mailcore::imap::ServerSettings {
        host: "127.0.0.1".into(),
        port,
        security: "none".into(),
        username: ME.into(),
    }
}

/// A second client ("another device") on the same mailbox.
async fn other(port: u16) -> Conn {
    mailcore::imap::conn::connect(&settings(port), PASSWORD).await.unwrap()
}

async fn append(port: u16, folder: &str, raw: &str, flags: Option<&str>) {
    let mut c = other(port).await;
    c.session.append(folder, flags, None, raw.as_bytes()).await.unwrap();
    c.session.logout().await.unwrap();
}

fn mail(subject: &str, mid: &str) -> String {
    format!(
        "From: Bernd <bernd@example.com>\r\nTo: {ME}\r\nSubject: {subject}\r\nDate: Mon, 12 Oct 2026 09:00:00 +0200\r\nMessage-ID: <{mid}>\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nText zu {subject}\r\n"
    )
}

/// Mailbox names on the server (unescaped), via a raw LIST.
async fn server_folders(port: u16) -> Vec<String> {
    let mut c = other(port).await;
    let names: Vec<async_imap::types::Name> = c.session.list(Some(""), Some("*")).await.unwrap().try_collect().await.unwrap();
    let out = names.iter().map(|n| mailcore::imap::conn::unescape_name(n.name())).collect();
    drop(names);
    let _ = c.session.logout().await;
    out
}

fn input(dv_port: u16, smtp_port: u16) -> Value {
    json!({
        "email": ME, "displayName": "Anna Beispiel",
        "imap": {"host": "127.0.0.1", "port": dv_port, "security": "none", "username": ME},
        "smtp": {"host": "127.0.0.1", "port": smtp_port, "security": "none", "username": ME},
        "password": PASSWORD, "smtpPassword": null
    })
}

fn service(dir: &tempfile::TempDir) -> Arc<Service> {
    Service::new(
        ServiceOptions {
            data_dir: dir.path().to_path_buf(),
            graph_base: "http://127.0.0.1:9/v1.0".into(),
            authority: "http://127.0.0.1:9".into(),
            demo: false,
            token_store: Arc::new(MemoryTokenStore::default()),
        },
        Arc::new(NullHost::default()),
    )
    .unwrap()
}

async fn setup_with(opts: Options, seed: &[(&str, &str)]) -> Option<Ctx> {
    let dv = Dovecot::start_with(opts)?;
    for (subject, mid) in seed {
        append(dv.port, "INBOX", &mail(subject, mid), None).await;
    }
    let sink = graph_fake::smtp::spawn_sink(PASSWORD).await;
    let dir = tempfile::tempdir().unwrap();
    let svc = service(&dir);
    let acc: Account = call(&svc, "add_imap_account", json!({"input": input(dv.port, sink.port)})).await;
    Some(Ctx {
        svc,
        dv,
        sink,
        _dir: dir,
        account: acc.id,
    })
}

async fn setup() -> Option<Ctx> {
    setup_with(Options::default(), &[("Erste", "e1@example.com"), ("Zweite", "e2@example.com")]).await
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
            json!({"mailboxId": self.account, "folderId": folder, "sortBy": "received", "sortDesc": true, "limit": 1000}),
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
    async fn search(&self, text: &str, folder: Option<&str>) -> Vec<String> {
        let r: Vec<MessageSummary> = call(
            &self.svc,
            "search_messages",
            json!({"mailboxId": self.account, "text": text, "folderId": folder, "field": "all", "hasAttachments": false}),
        )
        .await;
        let mut v: Vec<String> = r.into_iter().map(|m| m.subject).collect();
        v.sort();
        v
    }
    async fn send(&self, draft: Value) {
        let _: Value = call(&self.svc, "send_draft", json!({ "draft": draft })).await;
    }
}

// ---- outgoing: header injection & validation ----

#[tokio::test]
async fn line_breaks_in_header_fields_never_crash_or_inject() {
    let Some(ctx) = setup().await else { return };
    let b64 = base64::engine::general_purpose::STANDARD.encode(b"inhalt");
    let draft = json!({"id": null, "mailboxId": ctx.account,
        "to": [{"name": "Evil\r\nX-Injiziert: ja", "address": ME}],
        "cc": [{"name": "Zeile\nBcc: opfer@example.com", "address": "carl@example.com"}],
        "bcc": [{"name": "Geheim\r\n", "address": "geheim@example.com"}],
        "subject": "Betreff\r\nX-Injiziert2: ja",
        "bodyHtml": "<p>Hallo</p>",
        "newAttachments": [{"name": "datei\r\nX-Injiziert3: ja.txt", "contentType": "text/plain\r\nX-Injiziert4: ja", "contentBase64": b64}]});

    // Saving (APPEND to Drafts) must not crash either.
    let saved: Draft = call(&ctx.svc, "save_draft", json!({"draft": draft})).await;
    assert_eq!(saved.to[0].address, ME);
    assert!(!saved.to[0].name.contains('\n'));
    assert!(!saved.subject.contains('\n'));

    ctx.send(draft.clone()).await;
    let msgs = ctx.sink.messages();
    assert_eq!(msgs.len(), 1);
    let raw = &msgs[0].raw;
    for line in raw.split("\r\n") {
        let l = line.to_ascii_lowercase();
        assert!(!l.starts_with("x-injiziert"), "{raw}");
        assert!(!l.starts_with("bcc:"), "{raw}");
    }
    assert!(msgs[0].rcpts.contains(&"geheim@example.com".to_string()));
    assert!(!msgs[0].rcpts.contains(&"opfer@example.com".to_string()));
    // The copy in Sent is intact.
    let sent = ctx.folder("sentitems").await;
    let m = ctx.list(&sent.id).await.items.into_iter().next().expect("copy in Sent");
    assert_eq!(m.subject, "Betreff X-Injiziert2: ja");
    // The service survived: a normal call still works.
    let _: Vec<Folder> = call(&ctx.svc, "sync_folders", json!({"mailboxId": ctx.account})).await;
}

#[tokio::test]
async fn invalid_recipients_are_rejected_in_german() {
    let Some(ctx) = setup().await else { return };
    for bad in ["kein-at", "a@", "@b.de", "a b@c.de", "x@y.de\r\nBcc: z@z.de"] {
        let err = call_err(
            &ctx.svc,
            "send_draft",
            json!({"draft": {"id": null, "mailboxId": ctx.account, "to": [{"name": "", "address": bad}], "subject": "x", "bodyHtml": ""}}),
        )
        .await;
        assert!(err.contains("Ungültige Adresse"), "{bad:?}: {err}");
    }
    let err = call_err(&ctx.svc, "send_draft", json!({"draft": {"id": null, "mailboxId": ctx.account, "to": [], "subject": "x"}})).await;
    assert!(err.contains("Empfänger"), "{err}");
    assert!(ctx.sink.messages().is_empty());
}

// ---- replies ----

#[tokio::test]
async fn reply_headers_and_recipients() {
    let Some(ctx) = setup_with(Options::default(), &[]).await else { return };
    let inbox = ctx.folder("inbox").await;
    // No Message-ID at all.
    append(ctx.dv.port, "INBOX", "From: Ohne <ohne@example.com>\r\nTo: anna@example.org\r\nSubject: Ohne ID\r\n\r\nx\r\n", None).await;
    // Only In-Reply-To (no References), recipients in odd case and duplicated.
    append(
        ctx.dv.port,
        "INBOX",
        "From: Bernd <bernd@example.com>\r\nReply-To: bernd@example.com\r\nTo: ANNA@Example.org, carl@example.com, Carl <CARL@example.com>\r\nCc: anna@example.org, dora@example.com\r\nSubject: Re: Kette\r\nMessage-ID: <k2@example.com>\r\nIn-Reply-To: <k1@example.com>\r\n\r\nx\r\n",
        None,
    )
    .await;
    // My own sent mail.
    append(ctx.dv.port, "INBOX", &format!("From: Anna <{ME}>\r\nTo: Erik <erik@example.com>\r\nSubject: Von mir\r\nMessage-ID: <own@example.org>\r\n\r\nx\r\n"), None).await;
    ctx.sync(&inbox.id).await;
    let drafts = ctx.folder("drafts").await;

    let m = ctx.by_subject(&inbox.id, "Ohne ID").await;
    let d: Draft = call(&ctx.svc, "create_draft", json!({"mailboxId": ctx.account, "mode": "reply", "sourceMessageId": m.id})).await;
    assert_eq!(d.to[0].address, "ohne@example.com");
    assert_eq!(d.subject, "AW: Ohne ID");

    let m = ctx.by_subject(&inbox.id, "Re: Kette").await;
    let d: Draft = call(&ctx.svc, "create_draft", json!({"mailboxId": ctx.account, "mode": "replyAll", "sourceMessageId": m.id})).await;
    let all: Vec<String> = d.to.iter().chain(&d.cc).map(|a| a.address.to_lowercase()).collect();
    assert!(!all.contains(&ME.to_string()), "{all:?}");
    assert_eq!(all.iter().filter(|a| *a == "carl@example.com").count(), 1, "{all:?}");
    assert_eq!(all.iter().filter(|a| *a == "bernd@example.com").count(), 1, "{all:?}");
    assert!(all.contains(&"dora@example.com".to_string()));
    assert_eq!(d.subject, "AW: Kette");
    let raw = String::from_utf8_lossy(&raw_of(ctx.dv.port, &drafts.id, "AW: Kette").await).into_owned();
    let msg = mail_parser::MessageParser::default().parse(raw.as_bytes()).unwrap();
    assert_eq!(msg.header_raw("In-Reply-To").map(str::trim), Some("<k2@example.com>"));
    let refs: Vec<&str> = msg.header_raw("References").unwrap_or("").split_whitespace().collect();
    assert_eq!(refs, ["<k1@example.com>", "<k2@example.com>"], "{raw}");

    let m = ctx.by_subject(&inbox.id, "Von mir").await;
    let d: Draft = call(&ctx.svc, "create_draft", json!({"mailboxId": ctx.account, "mode": "reply", "sourceMessageId": m.id})).await;
    assert_eq!(d.to.iter().map(|a| a.address.as_str()).collect::<Vec<_>>(), ["erik@example.com"]);
}

async fn raw_of(port: u16, folder: &str, subject: &str) -> Vec<u8> {
    let mut c = other(port).await;
    c.session.select(folder).await.unwrap();
    let fetches: Vec<_> = c.session.fetch("1:*", "(BODY.PEEK[])").await.unwrap().try_collect().await.unwrap();
    let found = fetches
        .iter()
        .filter_map(|f| f.body().map(<[u8]>::to_vec))
        .find(|b| {
            mail_parser::MessageParser::default()
                .parse(b)
                .and_then(|m| m.subject().map(|s| s == subject))
                .unwrap_or(false)
        })
        .unwrap_or_else(|| panic!("{subject} not on server"));
    drop(fetches);
    let _ = c.session.logout().await;
    found
}

// ---- drafts ----

#[tokio::test]
async fn overlapping_draft_saves_leave_one_draft() {
    let Some(ctx) = setup().await else { return };
    let drafts = ctx.folder("drafts").await;
    let first: Draft = call(
        &ctx.svc,
        "save_draft",
        json!({"draft": {"id": null, "mailboxId": ctx.account, "subject": "Entwurf 0", "bodyHtml": "<p>a</p>"}}),
    )
    .await;
    let id = first.id.clone().unwrap();
    let mut tasks = Vec::new();
    for i in 1..=3 {
        let svc = ctx.svc.clone();
        let d = json!({"draft": {"id": id, "mailboxId": ctx.account, "subject": format!("Entwurf {i}"), "bodyHtml": "<p>b</p>"}});
        tasks.push(tokio::spawn(async move { svc.dispatch("save_draft", d).await }));
    }
    let mut ok = 0;
    for t in tasks {
        if t.await.unwrap().is_ok() {
            ok += 1;
        }
    }
    assert!(ok >= 1);
    ctx.sync(&drafts.id).await;
    let page = ctx.list(&drafts.id).await;
    assert_eq!(page.total, 1, "{:?}", page.items.iter().map(|m| &m.subject).collect::<Vec<_>>());
    let mut c = other(ctx.dv.port).await;
    let mb = c.session.select(&drafts.id).await.unwrap();
    assert_eq!(mb.exists, 1);
}

#[tokio::test]
async fn draft_with_inline_images_sharing_a_cid_prefix() {
    let Some(ctx) = setup_with(Options::default(), &[]).await else { return };
    let enc = |b: &[u8]| base64::engine::general_purpose::STANDARD.encode(b);
    let raw = format!(
        "From: Bernd <bernd@example.com>\r\nTo: {ME}\r\nSubject: Bilder\r\nMessage-ID: <img@example.com>\r\nContent-Type: multipart/related; boundary=r\r\n\r\n\
--r\r\nContent-Type: text/html\r\n\r\n<p><img src=\"cid:img1\"><img src=\"cid:img10\"></p>\r\n\
--r\r\nContent-Type: image/png\r\nContent-ID: <img1>\r\nContent-Transfer-Encoding: base64\r\n\r\n{}\r\n\
--r\r\nContent-Type: image/png\r\nContent-ID: <img10>\r\nContent-Transfer-Encoding: base64\r\n\r\n{}\r\n--r--\r\n",
        enc(b"EINS"),
        enc(b"ZEHN")
    );
    append(ctx.dv.port, "INBOX", &raw, None).await;
    let inbox = ctx.folder("inbox").await;
    ctx.sync(&inbox.id).await;
    let m = ctx.by_subject(&inbox.id, "Bilder").await;
    let d: MessageDetail = call(&ctx.svc, "get_message", json!({"mailboxId": ctx.account, "messageId": m.id})).await;
    assert!(d.body_html.contains(&enc(b"EINS")) && d.body_html.contains(&enc(b"ZEHN")), "{}", d.body_html);
    let fwd: Draft = call(&ctx.svc, "create_draft", json!({"mailboxId": ctx.account, "mode": "forward", "sourceMessageId": m.id})).await;
    assert!(fwd.body_html.contains(&enc(b"EINS")) && fwd.body_html.contains(&enc(b"ZEHN")), "{}", fwd.body_html);
    assert!(fwd.attachments.is_empty(), "{:?}", fwd.attachments);
    // Save and send keep both images inline.
    let saved: Draft = call(
        &ctx.svc,
        "save_draft",
        json!({"draft": {"id": fwd.id, "mailboxId": ctx.account, "mode": "forward", "to": [{"name": "", "address": "x@example.com"}],
            "subject": fwd.subject, "bodyHtml": fwd.body_html}}),
    )
    .await;
    assert!(saved.body_html.contains(&enc(b"EINS")) && saved.body_html.contains(&enc(b"ZEHN")), "{}", saved.body_html);
    ctx.send(json!({"id": saved.id, "mailboxId": ctx.account, "mode": "forward", "to": [{"name": "", "address": "x@example.com"}],
        "subject": saved.subject, "bodyHtml": saved.body_html}))
    .await;
    let sent = &ctx.sink.messages()[0].raw;
    assert_eq!(sent.matches("Content-ID:").count(), 2, "{sent}");
    let mut inline: Vec<(String, Vec<u8>)> = mailcore::imap::mime::all_attachments(sent.as_bytes())
        .into_iter()
        .map(|(_, _, _, bytes, cid)| (cid.unwrap_or_default(), bytes))
        .collect();
    inline.sort();
    assert_eq!(inline, [("img1".to_string(), b"EINS".to_vec()), ("img10".to_string(), b"ZEHN".to_vec())]);
}

// ---- folder names ----

#[tokio::test]
async fn folder_names_with_quotes_backslashes_and_emoji() {
    let Some(ctx) = setup().await else { return };
    let inbox = ctx.folder("inbox").await;
    let parent: Folder = call(&ctx.svc, "create_folder", json!({"mailboxId": ctx.account, "parentId": null, "name": "Prüfung Ä ö ü ß & Co"})).await;
    assert_eq!(parent.display_name, "Prüfung Ä ö ü ß & Co");
    let child: Folder = call(&ctx.svc, "create_folder", json!({"mailboxId": ctx.account, "parentId": parent.id, "name": "Unter € 📁"})).await;
    assert_eq!(child.display_name, "Unter € 📁");
    assert_eq!(child.parent_id.as_deref(), Some(parent.id.as_str()));

    let renamed: Folder = call(&ctx.svc, "rename_folder", json!({"mailboxId": ctx.account, "folderId": child.id, "name": "Kunde \"Müller\""})).await;
    assert_eq!(renamed.display_name, "Kunde \"Müller\"");
    let back: Folder = call(&ctx.svc, "create_folder", json!({"mailboxId": ctx.account, "parentId": parent.id, "name": "Back\\slash"})).await;
    assert_eq!(back.display_name, "Back\\slash");
    assert!(server_folders(ctx.dv.port).await.contains(&renamed.id));

    // Messages can be moved/copied in, listed, read, counted and searched.
    let a = ctx.by_subject(&inbox.id, "Erste").await;
    let b = ctx.by_subject(&inbox.id, "Zweite").await;
    let _: Value = call(&ctx.svc, "move_messages", json!({"mailboxId": ctx.account, "ids": [a.id], "destFolderId": renamed.id})).await;
    let _: Value = call(&ctx.svc, "copy_messages", json!({"mailboxId": ctx.account, "ids": [b.id], "destFolderId": back.id})).await;
    let moved = ctx.by_subject(&renamed.id, "Erste").await;
    let d: MessageDetail = call(&ctx.svc, "get_message", json!({"mailboxId": ctx.account, "messageId": moved.id})).await;
    assert!(d.body_html.contains("Text zu Erste"));
    let _: Value = call(&ctx.svc, "set_read", json!({"mailboxId": ctx.account, "ids": [moved.id], "isRead": false})).await;
    let folders: Vec<Folder> = call(&ctx.svc, "sync_folders", json!({"mailboxId": ctx.account})).await;
    let f = folders.iter().find(|f| f.id == renamed.id).unwrap();
    assert_eq!((f.total_count, f.unread_count), (1, 1));
    assert_eq!(folders.iter().find(|f| f.id == back.id).unwrap().total_count, 1);
    assert_eq!(ctx.search("Erste", Some(&renamed.id)).await, ["Erste"]);
    assert!(ctx.search("Text", None).await.contains(&"Erste".to_string()));

    // Deleting the parent removes all children on the server too.
    let _: Value = call(&ctx.svc, "delete_folder", json!({"mailboxId": ctx.account, "folderId": parent.id})).await;
    let left = server_folders(ctx.dv.port).await;
    assert!(!left.iter().any(|n| n.starts_with(&parent.id)), "{left:?}");
    assert!(!ctx.folders().await.iter().any(|f| f.id.starts_with(&parent.id)));
}

#[tokio::test]
async fn folder_name_validation_and_protected_folders() {
    let Some(ctx) = setup().await else { return };
    for (name, expect) in [("", "leer"), ("a/b", "\"/\""), ("Zeile\r\nzwei", "Zeilenumbrüche"), ("Stern*", "\"*\"")] {
        let err = call_err(&ctx.svc, "create_folder", json!({"mailboxId": ctx.account, "parentId": null, "name": name})).await;
        assert!(err.contains(expect), "{name:?}: {err}");
    }
    let inbox = ctx.folder("inbox").await;
    let sent = ctx.folder("sentitems").await;
    for f in [&inbox, &sent] {
        let err = call_err(&ctx.svc, "rename_folder", json!({"mailboxId": ctx.account, "folderId": f.id, "name": "Anders"})).await;
        assert!(err.contains("Standardordner"), "{err}");
        let err = call_err(&ctx.svc, "delete_folder", json!({"mailboxId": ctx.account, "folderId": f.id})).await;
        assert!(err.contains("Standardordner"), "{err}");
    }
    assert_eq!(ctx.list(&inbox.id).await.total, 2);
    // Existing name → server error, nothing lost.
    let _: Folder = call(&ctx.svc, "create_folder", json!({"mailboxId": ctx.account, "parentId": null, "name": "Doppelt"})).await;
    let err = call_err(&ctx.svc, "create_folder", json!({"mailboxId": ctx.account, "parentId": null, "name": "Doppelt"})).await;
    assert!(!err.is_empty());
    // Renaming to the same name is a no-op.
    let same: Folder = call(&ctx.svc, "rename_folder", json!({"mailboxId": ctx.account, "folderId": "Doppelt", "name": "Doppelt"})).await;
    assert_eq!(same.id, "Doppelt");
    // Deeply nested folders.
    let mut parent: Option<String> = None;
    for i in 0..8 {
        let f: Folder = call(&ctx.svc, "create_folder", json!({"mailboxId": ctx.account, "parentId": parent, "name": format!("Ebene {i}")})).await;
        parent = Some(f.id);
    }
    let deepest = parent.unwrap();
    assert_eq!(deepest.matches('/').count(), 7);
    let _: Value = call(&ctx.svc, "delete_folder", json!({"mailboxId": ctx.account, "folderId": "Ebene 0"})).await;
    assert!(!server_folders(ctx.dv.port).await.iter().any(|n| n.starts_with("Ebene")));
}

// ---- other clients ----

#[tokio::test]
async fn folders_removed_elsewhere_do_not_break_sync_or_search() {
    let Some(ctx) = setup().await else { return };
    let proj: Folder = call(&ctx.svc, "create_folder", json!({"mailboxId": ctx.account, "parentId": null, "name": "Projekt X"})).await;
    let inbox = ctx.folder("inbox").await;
    let a = ctx.by_subject(&inbox.id, "Erste").await;
    let _: Value = call(&ctx.svc, "copy_messages", json!({"mailboxId": ctx.account, "ids": [a.id], "destFolderId": proj.id})).await;
    // Another client deletes it.
    let mut c = other(ctx.dv.port).await;
    c.session.delete(&proj.id).await.unwrap();
    c.session.logout().await.unwrap();

    // Global search still finds the rest.
    assert_eq!(ctx.search("Zweite", None).await, ["Zweite"]);
    assert!(!ctx.folders().await.iter().any(|f| f.id == proj.id), "stale folder still listed");
    // Syncing the vanished folder is not an error (it disappears).
    let _: Folder = call(&ctx.svc, "create_folder", json!({"mailboxId": ctx.account, "parentId": null, "name": "Projekt Y"})).await;
    let mut c = other(ctx.dv.port).await;
    c.session.delete("Projekt Y").await.unwrap();
    c.session.logout().await.unwrap();
    let r = ctx.sync("Projekt Y").await;
    assert_eq!((r.added, r.updated, r.removed), (0, 0, 0));
    assert!(!ctx.folders().await.iter().any(|f| f.id == "Projekt Y"));
    // Search in that one folder reports the problem.
    let err = call_err(
        &ctx.svc,
        "search_messages",
        json!({"mailboxId": ctx.account, "text": "x", "folderId": "Projekt Y", "field": "all", "hasAttachments": false}),
    )
    .await;
    assert!(!err.is_empty());
}

#[tokio::test]
async fn messages_expunged_elsewhere() {
    let Some(ctx) = setup().await else { return };
    let inbox = ctx.folder("inbox").await;
    let a = ctx.by_subject(&inbox.id, "Erste").await;
    let mut c = other(ctx.dv.port).await;
    c.session.select("INBOX").await.unwrap();
    let _: Vec<_> = c.session.store("1:*", "+FLAGS (\\Deleted)").await.unwrap().try_collect().await.unwrap();
    let _: Vec<_> = c.session.expunge().await.unwrap().try_collect().await.unwrap();
    c.session.logout().await.unwrap();

    let err = call_err(&ctx.svc, "get_message", json!({"mailboxId": ctx.account, "messageId": a.id})).await;
    assert!(err.contains("Nicht gefunden"), "{err}");
    let err = call_err(&ctx.svc, "create_draft", json!({"mailboxId": ctx.account, "mode": "reply", "sourceMessageId": a.id})).await;
    assert!(err.contains("Nicht gefunden"), "{err}");
    let err = call_err(
        &ctx.svc,
        "download_attachment",
        json!({"mailboxId": ctx.account, "messageId": a.id, "attachmentId": "0", "action": "open"}),
    )
    .await;
    assert!(err.contains("Nicht gefunden"), "{err}");
    // Flag changes, moves and deletes of vanished mails are harmless.
    let _: Value = call(&ctx.svc, "set_read", json!({"mailboxId": ctx.account, "ids": [a.id], "isRead": true})).await;
    let _: Value = call(&ctx.svc, "set_flag", json!({"mailboxId": ctx.account, "ids": [a.id], "flag": "flagged"})).await;
    let _: Value = call(&ctx.svc, "archive_messages", json!({"mailboxId": ctx.account, "ids": [a.id]})).await;
    let b = ctx.by_subject(&inbox.id, "Zweite").await;
    let _: Value = call(&ctx.svc, "delete_messages", json!({"mailboxId": ctx.account, "ids": [b.id], "permanent": true})).await;
    let r = ctx.sync(&inbox.id).await;
    assert_eq!(r.added, 0);
    assert_eq!(ctx.list(&inbox.id).await.total, 0);
    let archive = ctx.folder("archive").await;
    assert_eq!(ctx.list(&archive.id).await.total, 0);
}

#[tokio::test]
async fn uidvalidity_change_resets_cache_and_protects_actions() {
    let Some(ctx) = setup().await else { return };
    let f: Folder = call(&ctx.svc, "create_folder", json!({"mailboxId": ctx.account, "parentId": null, "name": "Neuaufbau"})).await;
    append(ctx.dv.port, &f.id, &mail("Alt", "alt@example.com"), None).await;
    ctx.sync(&f.id).await;
    let old = ctx.by_subject(&f.id, "Alt").await;
    let (uv_old, uid_old, _) = mailcore::imap::parse_message_id(&old.id).unwrap();

    // Another client recreates the folder: same name, new UIDVALIDITY, and
    // a different message that gets the same UID.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let mut c = other(ctx.dv.port).await;
        c.session.delete(&f.id).await.unwrap();
        c.session.create(&f.id).await.unwrap();
        c.session.append(&f.id, None, None, mail("Neu", "neu@example.com").as_bytes()).await.unwrap();
        let mb = c.session.select(&f.id).await.unwrap();
        c.session.logout().await.unwrap();
        if mb.uid_validity != Some(uv_old) || Instant::now() > deadline {
            assert_ne!(mb.uid_validity, Some(uv_old));
            break;
        }
        tokio::time::sleep(Duration::from_millis(1100)).await;
    }

    // Actions with the stale id must not touch the new message.
    let err = call_err(&ctx.svc, "delete_messages", json!({"mailboxId": ctx.account, "ids": [old.id], "permanent": true})).await;
    assert!(err.contains("neu aufgebaut"), "{err}");
    let err = call_err(&ctx.svc, "set_read", json!({"mailboxId": ctx.account, "ids": [old.id], "isRead": true})).await;
    assert!(err.contains("neu aufgebaut"), "{err}");
    let mut c = other(ctx.dv.port).await;
    let mb = c.session.select(&f.id).await.unwrap();
    assert_eq!(mb.exists, 1, "new message must survive");
    let _ = c.session.logout().await;

    let _ = ctx.sync(&f.id).await;
    let page = ctx.list(&f.id).await;
    assert_eq!(page.items.iter().map(|m| m.subject.as_str()).collect::<Vec<_>>(), ["Neu"]);
    let (uv_new, uid_new, _) = mailcore::imap::parse_message_id(&page.items[0].id).unwrap();
    assert_ne!(uv_new, uv_old);
    let _ = (uid_old, uid_new);
}

#[tokio::test]
async fn flags_and_keywords_changed_elsewhere() {
    let Some(ctx) = setup().await else { return };
    let inbox = ctx.folder("inbox").await;
    let mut c = other(ctx.dv.port).await;
    c.session.select("INBOX").await.unwrap();
    let kw = mailcore::imap::category_keyword("Kunde Ä");
    let _: Vec<_> = c
        .session
        .store("1", format!("+FLAGS (\\Seen \\Flagged {kw} $due_kaputt $due_20261021T0600Z $Weird)"))
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    c.session.logout().await.unwrap();
    let r = ctx.sync(&inbox.id).await;
    assert_eq!(r.updated, 1);
    let m = ctx.by_subject(&inbox.id, "Erste").await;
    assert!(m.is_read);
    assert_eq!(m.flag, "flagged");
    assert_eq!(m.categories, ["Kunde Ä"]);
    // Idempotent: repeating the same actions changes nothing.
    for _ in 0..2 {
        let _: Value = call(&ctx.svc, "set_read", json!({"mailboxId": ctx.account, "ids": [m.id], "isRead": true})).await;
        let _: Value = call(&ctx.svc, "set_categories", json!({"mailboxId": ctx.account, "ids": [m.id], "categories": ["Kunde Ä", "Mit Leer zeichen"]})).await;
    }
    let r = ctx.sync(&inbox.id).await;
    assert_eq!((r.added, r.updated, r.removed), (0, 0, 0));
    let m = ctx.by_subject(&inbox.id, "Erste").await;
    let mut cats = m.categories.clone();
    cats.sort();
    assert_eq!(cats, ["Kunde Ä", "Mit Leer zeichen"]);
}

// ---- paging & empty folders ----

#[tokio::test]
async fn many_messages_page_with_load_older() {
    let Some(ctx) = setup_with(Options::default(), &[]).await else { return };
    let mut c = other(ctx.dv.port).await;
    for i in 0..650 {
        c.session.append("INBOX", Some("(\\Seen)"), None, mail(&format!("Nr {i:03}"), &format!("n{i}@example.com")).as_bytes()).await.unwrap();
    }
    c.session.logout().await.unwrap();
    let inbox = ctx.folder("inbox").await;
    ctx.sync(&inbox.id).await;
    let page = ctx.list(&inbox.id).await;
    assert_eq!(page.items.len(), 500);
    assert!(page.can_load_older);
    let n: i64 = call(&ctx.svc, "load_older", json!({"mailboxId": ctx.account, "folderId": inbox.id})).await;
    assert_eq!(n, 150);
    let page = ctx.list(&inbox.id).await;
    assert_eq!(page.items.len(), 650);
    assert!(!page.can_load_older);
    let n: i64 = call(&ctx.svc, "load_older", json!({"mailboxId": ctx.account, "folderId": inbox.id})).await;
    assert_eq!(n, 0);
    assert!(ctx.search("Nr", None).await.len() <= 100);
}

#[tokio::test]
async fn empty_folder_operations() {
    let Some(ctx) = setup_with(Options::default(), &[]).await else { return };
    let inbox = ctx.folder("inbox").await;
    assert_eq!(ctx.list(&inbox.id).await.total, 0);
    let r = ctx.sync(&inbox.id).await;
    assert_eq!((r.added, r.updated, r.removed), (0, 0, 0));
    let _: Value = call(&ctx.svc, "mark_folder_read", json!({"mailboxId": ctx.account, "folderId": inbox.id})).await;
    let _: Value = call(&ctx.svc, "empty_folder", json!({"mailboxId": ctx.account, "folderId": inbox.id})).await;
    let n: i64 = call(&ctx.svc, "load_older", json!({"mailboxId": ctx.account, "folderId": inbox.id})).await;
    assert_eq!(n, 0);
    assert!(ctx.search("irgendwas", None).await.is_empty());
    let _: Value = call(&ctx.svc, "set_read", json!({"mailboxId": ctx.account, "ids": [], "isRead": true})).await;
}

// ---- search ----

#[tokio::test]
async fn search_with_special_characters() {
    let Some(ctx) = setup_with(
        Options::default(),
        &[("Grüße aus Köln", "s1@example.com"), ("Zitat \"genau\" hier", "s2@example.com"), ("Pfad C:\\temp (Kopie)", "s3@example.com")],
    )
    .await
    else {
        return;
    };
    assert_eq!(ctx.search("Grüße", None).await, ["Grüße aus Köln"]);
    assert_eq!(ctx.search("\"genau\"", None).await, ["Zitat \"genau\" hier"]);
    assert_eq!(ctx.search("C:\\temp", None).await, ["Pfad C:\\temp (Kopie)"]);
    assert_eq!(ctx.search("(Kopie)", None).await, ["Pfad C:\\temp (Kopie)"]);
    for odd in ["\\", "\"", "*", "%", "{5}", "a\r\nb", ")(", "😀"] {
        let _ = ctx.search(odd, None).await;
    }
    assert!(ctx.search("   ", None).await.is_empty());
}

// ---- connections ----

#[tokio::test]
async fn dropped_connection_reconnects_transparently() {
    let Some(ctx) = setup().await else { return };
    let inbox = ctx.folder("inbox").await;
    ctx.dv.kill_sessions();
    let r = ctx.sync(&inbox.id).await;
    assert_eq!(r.removed, 0);
    let m = ctx.by_subject(&inbox.id, "Erste").await;
    ctx.dv.kill_sessions();
    let _: Value = call(&ctx.svc, "set_read", json!({"mailboxId": ctx.account, "ids": [m.id], "isRead": true})).await;
    assert!(ctx.by_subject(&inbox.id, "Erste").await.is_read);
}

async fn connect_err(port: u16, security: &str) -> String {
    let mut s = settings(port);
    s.security = security.into();
    match tokio::time::timeout(Duration::from_secs(40), mailcore::imap::conn::connect(&s, PASSWORD)).await {
        Ok(Ok(_)) => panic!("connect with {security} unexpectedly succeeded"),
        Ok(Err(e)) => e.to_string(),
        Err(_) => panic!("connect with {security} hangs"),
    }
}

#[tokio::test]
async fn connection_errors_are_clear() {
    let Some(dv) = Dovecot::start() else { return };
    // TLS against a plain server, STARTTLS where it is not offered.
    let e = connect_err(dv.port, "ssl").await;
    assert!(e.contains("TLS") || e.contains("Verbindung"), "{e}");
    let e = connect_err(dv.port, "starttls").await;
    assert!(!e.is_empty(), "{e}");
    // Unknown setting never falls back to plain text.
    let e = connect_err(dv.port, "SSL/TLS").await;
    assert!(e.contains("Unbekannte Verschlüsselung"), "{e}");
    // Wrong password.
    let mut s = settings(dv.port);
    s.security = "none".into();
    let e = mailcore::imap::conn::connect(&s, "falsch").await.err().unwrap().to_string();
    assert!(e.contains("Passwort falsch"), "{e}");
    // Nobody listening.
    let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let e = connect_err(free, "none").await;
    assert!(e.contains("Keine Verbindung"), "{e}");
    // A server that talks garbage.
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else { return };
            use tokio::io::AsyncWriteExt;
            let _ = s.write_all(b"\x00\xff garbage\r\n* BYE weg\r\n").await;
        }
    });
    let e = connect_err(port, "none").await;
    assert!(!e.is_empty());
}

#[tokio::test]
async fn silent_server_times_out() {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    tokio::spawn(async move {
        let mut keep = Vec::new();
        loop {
            let Ok((s, _)) = l.accept().await else { return };
            keep.push(s);
        }
    });
    let start = Instant::now();
    let e = connect_err(port, "none").await;
    assert!(e.contains("antwortet nicht"), "{e}");
    assert!(start.elapsed() < Duration::from_secs(30));
}

#[tokio::test]
async fn smtp_errors_are_clear() {
    let Some(ctx) = setup().await else { return };
    let mut i = input(ctx.dv.port, ctx.sink.port);
    i["smtp"]["security"] = json!("ssl");
    let r: Value = call(&ctx.svc, "test_imap_account", json!({"input": i})).await;
    assert_eq!(r["smtpOk"], false, "{r}");
    i["smtp"]["security"] = json!("starttls");
    let r: Value = call(&ctx.svc, "test_imap_account", json!({"input": i})).await;
    assert_eq!(r["smtpOk"], false, "{r}");
    i["smtp"]["security"] = json!("tls");
    let r: Value = call(&ctx.svc, "test_imap_account", json!({"input": i})).await;
    assert!(r["smtpError"].as_str().unwrap_or("").contains("Unbekannte Verschlüsselung"), "{r}");
    assert!(r["imapOk"].as_bool().unwrap(), "{r}");
}

// ---- server layout: "INBOX." namespace, no SPECIAL-USE ----

#[tokio::test]
async fn inbox_namespace_without_special_use() {
    let Some(dv) = Dovecot::start_with(Options {
        separator: '.',
        inbox_prefix: true,
    }) else {
        return;
    };
    let mut c = other(dv.port).await;
    for name in ["INBOX.Gesendete Objekte", "INBOX.Entw&APw-rfe", "INBOX.Papierkorb", "INBOX.Projekte", "INBOX.Projekte.Trash", "INBOX.Spam"] {
        c.session.create(name).await.unwrap();
    }
    c.session.append("INBOX", None, None, mail("Hallo", "h1@example.com").as_bytes()).await.unwrap();
    c.session.logout().await.unwrap();
    let sink = graph_fake::smtp::spawn_sink(PASSWORD).await;
    let dir = tempfile::tempdir().unwrap();
    let svc = service(&dir);
    let acc: Account = call(&svc, "add_imap_account", json!({"input": input(dv.port, sink.port)})).await;
    let ctx = Ctx {
        svc,
        dv,
        sink,
        _dir: dir,
        account: acc.id,
    };
    let folders = ctx.folders().await;
    let role = |id: &str| folders.iter().find(|f| f.id == id).and_then(|f| f.well_known.clone());
    assert_eq!(role("INBOX.Gesendete Objekte").as_deref(), Some("sentitems"));
    assert_eq!(role("INBOX.Entw&APw-rfe").as_deref(), Some("drafts"));
    assert_eq!(role("INBOX.Papierkorb").as_deref(), Some("deleteditems"));
    assert_eq!(role("INBOX.Spam").as_deref(), Some("junkemail"));
    assert_eq!(role("INBOX.Projekte.Trash"), None, "nested folder is not the trash");
    assert_eq!(folders.iter().find(|f| f.id == "INBOX.Entw&APw-rfe").unwrap().display_name, "Entwürfe");

    // Top-level folders land below INBOX; "." is rejected in names.
    let f: Folder = call(&ctx.svc, "create_folder", json!({"mailboxId": ctx.account, "parentId": null, "name": "Kunden"})).await;
    assert_eq!(f.id, "INBOX.Kunden");
    let err = call_err(&ctx.svc, "create_folder", json!({"mailboxId": ctx.account, "parentId": null, "name": "a.b"})).await;
    assert!(err.contains("\".\""), "{err}");

    // Snooze/archive create their folders below INBOX and keep the role.
    let inbox = ctx.folder("inbox").await;
    let m = ctx.by_subject(&inbox.id, "Hallo").await;
    let until = (chrono::Utc::now() + chrono::Duration::days(1)).to_rfc3339();
    let _: Value = call(&ctx.svc, "snooze_messages", json!({"mailboxId": ctx.account, "ids": [m.id], "until": until})).await;
    let snoozed = ctx.folder("snoozed").await;
    assert_eq!(snoozed.id, "INBOX.Wiedervorlage");
    let _: Vec<Folder> = call(&ctx.svc, "sync_folders", json!({"mailboxId": ctx.account})).await;
    assert_eq!(ctx.folder("snoozed").await.id, "INBOX.Wiedervorlage");
    let s = ctx.by_subject(&snoozed.id, "Hallo").await;
    assert!(s.due_at.is_some());
    let _: Value = call(&ctx.svc, "archive_messages", json!({"mailboxId": ctx.account, "ids": [s.id]})).await;
    let archive = ctx.folder("archive").await;
    assert_eq!(archive.id, "INBOX.Archive");
    ctx.by_subject(&archive.id, "Hallo").await;

    // Sending stores the copy in the name-detected Sent folder.
    ctx.send(json!({"id": null, "mailboxId": ctx.account, "to": [{"name": "", "address": "x@example.com"}], "subject": "Gesendet?", "bodyHtml": ""}))
        .await;
    ctx.by_subject("INBOX.Gesendete Objekte", "Gesendet?").await;
}
