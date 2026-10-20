//! Security tests of the service layer against the fake Graph server:
//! attachments, local files, ids that try to escape their Graph path.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use base64::Engine;
use mailcore::model::*;
use mailcore::tokens::MemoryTokenStore;
use mailcore::{Host, Service, ServiceOptions};
use serde_json::{json, Value};

/// Host that records what the service asks the OS to open.
struct RecordingHost {
    opened: Mutex<Vec<PathBuf>>,
    urls: Mutex<Vec<String>>,
    save_dir: PathBuf,
}

#[async_trait]
impl Host for RecordingHost {
    fn emit(&self, _event: &str, _payload: Value) {}
    fn open_url(&self, url: &str) -> mailcore::Result<()> {
        self.urls.lock().unwrap().push(url.to_string());
        Ok(())
    }
    fn open_path(&self, path: &Path) -> mailcore::Result<()> {
        self.opened.lock().unwrap().push(path.to_path_buf());
        Ok(())
    }
    async fn save_dialog(&self, suggested_name: &str) -> Option<PathBuf> {
        Some(self.save_dir.join(suggested_name))
    }
    fn notify(&self, _title: &str, _body: &str) {}
    fn open_window(&self, _kind: &str, _query: &str) -> mailcore::Result<()> {
        Ok(())
    }
}

struct Ctx {
    svc: Arc<Service>,
    host: Arc<RecordingHost>,
    mailbox: String,
    dir: tempfile::TempDir,
    _fake: graph_fake::FakeServer,
}

async fn setup() -> Ctx {
    let fake = graph_fake::spawn(graph_fake::FakeOptions {
        seed_demo: true,
        now: chrono::Utc::now(),
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let host = Arc::new(RecordingHost {
        opened: Mutex::default(),
        urls: Mutex::default(),
        save_dir: dir.path().join("downloads"),
    });
    std::fs::create_dir_all(&host.save_dir).unwrap();
    let svc = Service::new(
        ServiceOptions {
            data_dir: dir.path().join("data"),
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
    Ctx {
        mailbox: accounts[0].mailboxes[0].id.clone(),
        svc,
        host,
        dir,
        _fake: fake,
    }
}

async fn call<T: serde::de::DeserializeOwned>(svc: &Arc<Service>, cmd: &str, args: Value) -> T {
    let v = svc
        .dispatch(cmd, args.clone())
        .await
        .unwrap_or_else(|e| panic!("{cmd}({args}) failed: {e}"));
    serde_json::from_value(v).unwrap()
}

/// Saves a draft carrying one attachment per name; returns (draft id, attachments).
async fn draft_with(ctx: &Ctx, names: &[&str]) -> (String, Vec<Attachment>) {
    let b64 = base64::engine::general_purpose::STANDARD.encode(b"MZ fake binary");
    let atts: Vec<Value> = names
        .iter()
        .map(|n| json!({"name": n, "contentType": "application/octet-stream", "contentBase64": b64}))
        .collect();
    let d: Draft = call(
        &ctx.svc,
        "save_draft",
        json!({"draft": {"mailboxId": ctx.mailbox, "subject": "Anhänge", "newAttachments": atts}}),
    )
    .await;
    (d.id.unwrap(), d.attachments)
}

#[tokio::test]
async fn executable_attachments_are_never_opened() {
    let ctx = setup().await;
    let (id, atts) = draft_with(&ctx, &["rechnung.pdf.exe", "skript.js.", "rechnung\u{202E}fdp.exe", "bericht.pdf"]).await;
    for a in &atts {
        let r = ctx
            .svc
            .dispatch(
                "download_attachment",
                json!({"mailboxId": ctx.mailbox, "messageId": id, "attachmentId": a.id, "action": "open"}),
            )
            .await;
        if a.name == "bericht.pdf" {
            r.unwrap();
        } else {
            let e = r.expect_err(&a.name).to_string();
            assert!(e.contains("nicht direkt geöffnet"), "{}: {e}", a.name);
        }
    }
    let opened = ctx.host.opened.lock().unwrap().clone();
    assert_eq!(opened.len(), 1, "{opened:?}");
    assert!(opened[0].ends_with("bericht.pdf"));

    // saving (after an explicit "Speichern unter…") stays possible
    let exe = atts.iter().find(|a| a.name == "rechnung.pdf.exe").unwrap();
    let p: Option<String> = call(
        &ctx.svc,
        "download_attachment",
        json!({"mailboxId": ctx.mailbox, "messageId": id, "attachmentId": exe.id, "action": "save"}),
    )
    .await;
    assert!(p.unwrap().ends_with("rechnung.pdf.exe"));
}

#[tokio::test]
async fn attachment_names_cannot_escape_the_target_directory() {
    let ctx = setup().await;
    let (id, atts) = draft_with(&ctx, &["../../../evil.txt", "/etc/cron.d/x", "CON", "nul.txt", "a\0b.txt", "..", " . "]).await;
    for a in &atts {
        let p: Option<String> = call(
            &ctx.svc,
            "download_attachment",
            json!({"mailboxId": ctx.mailbox, "messageId": id, "attachmentId": a.id, "action": "save"}),
        )
        .await;
        let p = PathBuf::from(p.unwrap());
        assert_eq!(p.parent().unwrap(), ctx.host.save_dir, "{:?} → {p:?}", a.name);
        let file = p.file_name().unwrap().to_string_lossy().into_owned();
        assert!(!file.starts_with('.'), "{file}");
        assert!(!["CON", "nul.txt"].contains(&file.as_str()), "{file}");
    }
    assert!(!ctx.dir.path().join("evil.txt").exists());
}

#[tokio::test]
async fn dot_segment_ids_cannot_reach_other_graph_paths() {
    let ctx = setup().await;
    let folders: Vec<Folder> = call(&ctx.svc, "list_folders", json!({"mailboxId": ctx.mailbox})).await;
    let inbox = folders.iter().find(|f| f.well_known.as_deref() == Some("inbox")).unwrap();
    let count = |svc: Arc<Service>, mb: String, f: String| async move {
        let _: Value = call(&svc, "sync_folder", json!({"mailboxId": mb, "folderId": f})).await;
        let p: MessagePage = call(&svc, "list_messages", json!({"mailboxId": mb, "folderId": f, "limit": 1000})).await;
        p.total
    };
    let before = count(ctx.svc.clone(), ctx.mailbox.clone(), inbox.id.clone()).await;
    assert!(before > 0);
    // "/me/mailFolders/../messages" would be normalised to "/me/messages":
    // emptying "that folder" would delete every message of the mailbox.
    for id in ["..", ".", "%2e%2e", ".%2E"] {
        let r = ctx.svc.dispatch("empty_folder", json!({"mailboxId": ctx.mailbox, "folderId": id})).await;
        assert!(r.is_err(), "empty_folder({id}) muss scheitern");
        let r = ctx
            .svc
            .dispatch("delete_messages", json!({"mailboxId": ctx.mailbox, "ids": [id], "permanent": true}))
            .await;
        assert!(r.is_err(), "delete_messages({id}) muss scheitern");
    }
    let after = count(ctx.svc.clone(), ctx.mailbox.clone(), inbox.id.clone()).await;
    assert_eq!(before, after);
}

#[cfg(unix)]
#[tokio::test]
async fn local_cache_is_private() {
    use std::os::unix::fs::PermissionsExt;
    let ctx = setup().await;
    let mode = std::fs::metadata(ctx.dir.path().join("data")).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o700);
}

#[tokio::test]
async fn only_web_links_reach_the_browser() {
    let ctx = setup().await;
    let _: Value = call(&ctx.svc, "open_url", json!({"url": "https://example.com/a?b=c"})).await;
    let _: Value = call(&ctx.svc, "open_url", json!({"url": "mailto:anna@example.com?subject=x"})).await;
    for url in ["file:///etc/passwd", " javascript:alert(1)", "vbscript:x", "smb://host/share", "ms-settings:", "HTTPS:/x", "httpx://a"] {
        assert!(ctx.svc.dispatch("open_url", json!({"url": url})).await.is_err(), "{url}");
    }
    assert_eq!(*ctx.host.urls.lock().unwrap(), ["https://example.com/a?b=c"]);
}
