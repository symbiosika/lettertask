//! "Sofort benachrichtigen" (IMAP IDLE) and the unattended background pass,
//! against a real Dovecot server.

mod support;

use std::sync::Arc;
use std::time::Duration;

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
    _sink: graph_fake::smtp::SmtpSink,
    _dir: tempfile::TempDir,
    account: String,
}

async fn call<T: serde::de::DeserializeOwned>(svc: &Arc<Service>, cmd: &str, args: Value) -> T {
    let v = svc.dispatch(cmd, args.clone()).await.unwrap_or_else(|e| panic!("{cmd}({args}) failed: {e}"));
    serde_json::from_value(v).unwrap_or_else(|e| panic!("{cmd}: cannot decode result: {e}"))
}

async fn setup() -> Option<Ctx> {
    setup_with(support::dovecot::Options::default()).await
}

async fn setup_with(opts: support::dovecot::Options) -> Option<Ctx> {
    let dv = Dovecot::start_with(opts)?;
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
    let input = json!({
        "email": ME, "displayName": "Anna Beispiel",
        "imap": {"host": "127.0.0.1", "port": dv.port, "security": "none", "username": ME},
        "smtp": {"host": "127.0.0.1", "port": sink.port, "security": "none", "username": ME},
        "password": PASSWORD, "smtpPassword": null
    });
    let acc: Account = call(&svc, "add_imap_account", json!({ "input": input })).await;
    Some(Ctx { svc, host, dv, _sink: sink, _dir: dir, account: acc.id })
}

impl Ctx {
    async fn folder(&self, wk: &str) -> Folder {
        let fs: Vec<Folder> = call(&self.svc, "list_folders", json!({"mailboxId": self.account})).await;
        fs.into_iter().find(|f| f.well_known.as_deref() == Some(wk)).unwrap()
    }
    async fn subjects(&self, folder: &str) -> Vec<String> {
        let page: MessagePage = call(
            &self.svc,
            "list_messages",
            json!({"mailboxId": self.account, "folderId": folder, "sortBy": "received", "sortDesc": true, "limit": 500}),
        )
        .await;
        page.items.into_iter().map(|m| m.subject).collect()
    }
    async fn set_push(&self, on: bool) {
        let mut c: Config = call(&self.svc, "get_config", json!({})).await;
        c.push_enabled = on;
        let _: Config = call(&self.svc, "save_config", json!({ "config": c })).await;
    }
    async fn deliver(&self, subject: &str) {
        let s = mailcore::imap::ServerSettings { host: "127.0.0.1".into(), port: self.dv.port, security: "none".into(), username: ME.into() };
        let mut c = mailcore::imap::conn::connect(&s, PASSWORD).await.unwrap();
        let raw = format!("From: Bernd <bernd@example.com>\r\nTo: {ME}\r\nSubject: {subject}\r\nMessage-ID: <{}@example.com>\r\n\r\nHallo\r\n", subject.len());
        c.session.append("INBOX", None, None, raw.as_bytes()).await.unwrap();
        let _ = c.session.logout().await;
    }
    fn state(&self) -> Option<&'static str> {
        self.svc.push_status().first().map(|p| p.state)
    }
}

/// Polls `f` for up to 10 s.
async fn eventually(mut f: impl AsyncFnMut() -> bool) -> bool {
    for _ in 0..100 {
        if f().await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

#[tokio::test]
async fn push_announces_new_mail_without_a_sync() {
    let Some(ctx) = setup().await else { return };
    let inbox = ctx.folder("inbox").await;
    // An empty inbox counts as first sync later, and those are not announced.
    ctx.deliver("Schon da").await;
    ctx.svc.sync_all(true).await;
    assert!(ctx.svc.push_status().is_empty(), "off by default");

    ctx.set_push(true).await;
    assert!(eventually(async || ctx.state() == Some("listening")).await, "{:?}", ctx.svc.push_status());

    ctx.deliver("Per Push zugestellt").await;
    assert!(
        eventually(async || ctx.subjects(&inbox.id).await.iter().any(|s| s == "Per Push zugestellt")).await,
        "the IDLE listener synced the inbox by itself"
    );
    let events = ctx.host.events.lock().unwrap().clone();
    assert!(events.iter().any(|(e, p)| e == "new-mail" && p["subject"] == "Per Push zugestellt"), "{events:?}");

    // A refresh (phone after Doze) renews IDLE and keeps listening.
    ctx.svc.refresh_push();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(eventually(async || ctx.state() == Some("listening")).await);
    ctx.deliver("Nach dem Auffrischen").await;
    assert!(eventually(async || ctx.subjects(&inbox.id).await.iter().any(|s| s == "Nach dem Auffrischen")).await);

    ctx.set_push(false).await;
    assert!(ctx.svc.push_status().is_empty(), "switching off stops the listener");
}

#[tokio::test]
async fn fokuszeit_holds_push_mail_back() {
    let Some(ctx) = setup().await else { return };
    let inbox = ctx.folder("inbox").await;
    ctx.deliver("Schon da").await;
    ctx.svc.sync_all(true).await;
    ctx.set_push(true).await;
    assert!(eventually(async || ctx.state() == Some("listening")).await);
    let _: Value = call(&ctx.svc, "set_focus", json!({"minutes": 60})).await;
    ctx.deliver("Während der Fokuszeit").await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(!ctx.subjects(&inbox.id).await.iter().any(|s| s == "Während der Fokuszeit"));
    assert!(!ctx.host.events.lock().unwrap().iter().any(|(e, _)| e == "new-mail"));
}

#[tokio::test]
async fn background_pass_and_next_wakeup() {
    let Some(ctx) = setup().await else { return };
    let inbox = ctx.folder("inbox").await;
    ctx.deliver("Später wieder").await;
    let run = ctx.svc.background_sync().await;
    assert!(run.ok && run.skipped.is_none(), "{run:?}");
    assert!(run.next_wakeup.is_none(), "nothing on Wiedervorlage yet");
    let page: MessagePage =
        call(&ctx.svc, "list_messages", json!({"mailboxId": ctx.account, "folderId": inbox.id, "limit": 50})).await;
    let m = page.items.iter().find(|m| m.subject == "Später wieder").expect("synced by the pass");

    let until = chrono::Utc::now() + chrono::Duration::hours(5);
    let _: Value = call(&ctx.svc, "snooze_messages", json!({"mailboxId": ctx.account, "ids": [m.id], "until": until.to_rfc3339()})).await;
    let due = ctx.svc.next_snooze_due().expect("due time from the cache");
    // IMAP keeps the due time in a keyword with minute precision.
    assert!((due - until).num_seconds().abs() <= 60, "{due} vs {until}");
    let run = ctx.svc.background_sync().await;
    assert!(run.next_wakeup.is_some());

    // Fokuszeit pauses the unattended pass like the desktop loop.
    let _: Value = call(&ctx.svc, "set_focus", json!({"minutes": 30})).await;
    let run = ctx.svc.background_sync().await;
    assert_eq!(run.skipped.as_deref(), Some("focus"));
    assert!(run.ok);
}

#[tokio::test]
async fn background_interval_is_clamped() {
    let Some(ctx) = setup().await else { return };
    let mut c: Config = call(&ctx.svc, "get_config", json!({})).await;
    assert_eq!(c.background_interval_mins, 15);
    assert!(!c.push_enabled);
    c.background_interval_mins = 5;
    let saved: Config = call(&ctx.svc, "save_config", json!({ "config": c.clone() })).await;
    assert_eq!(saved.background_interval_mins, 15, "the platform minimum");
    c.background_interval_mins = 0;
    let saved: Config = call(&ctx.svc, "save_config", json!({ "config": c })).await;
    assert_eq!(saved.background_interval_mins, 0, "never");
    assert!(ctx.host.events.lock().unwrap().iter().any(|(e, _)| e == "config-changed"));
}

#[tokio::test]
async fn server_without_idle_stays_on_the_interval() {
    let Some(ctx) = setup_with(support::dovecot::Options { minimal_caps: true, ..Default::default() }).await else { return };
    ctx.set_push(true).await;
    assert!(eventually(async || ctx.state() == Some("unsupported")).await, "{:?}", ctx.svc.push_status());
    let info = &ctx.svc.push_status()[0];
    assert!(info.message.as_deref().unwrap_or_default().contains("IDLE"));
    // the periodic pass still works
    assert!(ctx.svc.background_sync().await.ok);
}

#[tokio::test]
async fn background_plan_follows_settings_and_accounts() {
    let Some(ctx) = setup().await else { return };
    let plan = ctx.svc.background_plan();
    assert_eq!(plan.interval_mins, 15);
    assert!(!plan.push);
    ctx.set_push(true).await;
    assert!(ctx.svc.background_plan().push, "an IMAP account can listen");
    let _: Value = call(&ctx.svc, "remove_account", json!({"accountId": ctx.account})).await;
    let plan = ctx.svc.background_plan();
    assert_eq!(plan.interval_mins, 0, "nothing to sync without accounts");
    assert!(!plan.push, "no push service without an account that can use it");
}
