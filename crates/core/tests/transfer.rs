//! Moving and copying mails between mailboxes of different kinds:
//! fake Microsoft Graph account ↔ real Dovecot IMAP account in one service.

mod support;

use std::sync::Arc;

use mailcore::host::NullHost;
use mailcore::model::*;
use mailcore::tokens::MemoryTokenStore;
use mailcore::{Service, ServiceOptions};
use serde_json::{json, Value};
use support::dovecot::{Dovecot, PASSWORD};

const ME: &str = "anna@example.org";

async fn call<T: serde::de::DeserializeOwned>(svc: &Arc<Service>, cmd: &str, args: Value) -> T {
    let v = svc
        .dispatch(cmd, args.clone())
        .await
        .unwrap_or_else(|e| panic!("{cmd}({args}) failed: {e}"));
    serde_json::from_value(v).unwrap_or_else(|e| panic!("{cmd}: cannot decode result: {e}"))
}

async fn folder(svc: &Arc<Service>, mb: &str, wk: &str) -> Folder {
    let list: Vec<Folder> = call(svc, "list_folders", json!({"mailboxId": mb})).await;
    list.into_iter().find(|f| f.well_known.as_deref() == Some(wk)).unwrap_or_else(|| panic!("{wk} missing"))
}

async fn list(svc: &Arc<Service>, mb: &str, folder: &str) -> Vec<MessageSummary> {
    let _: Value = call(svc, "sync_folder", json!({"mailboxId": mb, "folderId": folder})).await;
    let page: MessagePage = call(
        svc,
        "list_messages",
        json!({"mailboxId": mb, "folderId": folder, "sortBy": "received", "sortDesc": true, "offset": 0, "limit": 500}),
    )
    .await;
    page.items
}

#[tokio::test]
async fn move_and_copy_between_graph_and_imap() {
    let Some(dv) = Dovecot::start() else {
        eprintln!("dovecot not installed – skipped");
        return;
    };
    let fake = graph_fake::spawn(graph_fake::FakeOptions { seed_demo: true, now: chrono::Utc::now() }).await;
    let sink = graph_fake::smtp::spawn_sink(PASSWORD).await;
    let dir = tempfile::tempdir().unwrap();
    let svc = Service::new(
        ServiceOptions {
            data_dir: dir.path().to_path_buf(),
            graph_base: format!("{}/v1.0", fake.base_url),
            authority: fake.base_url.clone(),
            demo: true,
            token_store: Arc::new(MemoryTokenStore::default()),
        },
        Arc::new(NullHost::default()),
    )
    .unwrap();
    svc.ensure_demo_account().await.unwrap();
    let graph_mb = {
        let accs: Vec<Account> = call(&svc, "list_accounts", json!({})).await;
        accs[0].mailboxes[0].id.clone()
    };
    let imap: Account = call(
        &svc,
        "add_imap_account",
        json!({"input": {
            "email": ME, "displayName": "Anna Beispiel",
            "imap": {"host": "127.0.0.1", "port": dv.port, "security": "none", "username": ME},
            "smtp": {"host": "127.0.0.1", "port": sink.port, "security": "none", "username": ME},
            "password": PASSWORD, "smtpPassword": null
        }}),
    )
    .await;
    let imap_mb = imap.mailboxes[0].id.clone();
    let g_inbox = folder(&svc, &graph_mb, "inbox").await;
    let i_inbox = folder(&svc, &imap_mb, "inbox").await;

    // Microsoft 365 → IMAP, move: arrives with its attachment, leaves the source.
    let src = list(&svc, &graph_mb, &g_inbox.id).await;
    let rechnung = src.iter().find(|m| m.subject == "Rechnung 2026-1043").expect("demo mail").clone();
    let r: Value = call(
        &svc,
        "transfer_messages",
        json!({"mailboxId": graph_mb, "ids": [rechnung.id], "destMailboxId": imap_mb, "destFolderId": i_inbox.id, "copy": false}),
    )
    .await;
    assert_eq!(r["count"], 1);
    let moved = list(&svc, &imap_mb, &i_inbox.id).await;
    let m = moved.iter().find(|m| m.subject == "Rechnung 2026-1043").expect("arrived in IMAP inbox");
    assert!(m.has_attachments, "attachment travels along");
    assert_eq!(m.from.as_ref().map(|a| a.address.clone()), rechnung.from.as_ref().map(|a| a.address.clone()));
    let d: MessageDetail = call(&svc, "get_message", json!({"mailboxId": imap_mb, "messageId": m.id})).await;
    assert!(d.attachments.iter().any(|a| a.name.ends_with(".pdf")), "{:?}", d.attachments);
    assert!(!list(&svc, &graph_mb, &g_inbox.id).await.iter().any(|m| m.id == rechnung.id), "gone from source");

    // IMAP → Microsoft 365, copy: a received (non-draft) mail with original date; source stays.
    let imap_mail = moved.iter().find(|m| m.subject == "Rechnung 2026-1043").unwrap().clone();
    let _: Value = call(
        &svc,
        "transfer_messages",
        json!({"mailboxId": imap_mb, "ids": [imap_mail.id], "destMailboxId": graph_mb, "destFolderId": g_inbox.id, "copy": true}),
    )
    .await;
    let back = list(&svc, &graph_mb, &g_inbox.id).await;
    let b = back.iter().find(|m| m.subject == "Rechnung 2026-1043").expect("copied back to Graph inbox");
    assert!(!b.is_draft, "not a draft");
    assert!(b.has_attachments);
    assert!(list(&svc, &imap_mb, &i_inbox.id).await.iter().any(|m| m.id == imap_mail.id), "copy keeps the source");
}
