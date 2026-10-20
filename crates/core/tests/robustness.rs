//! Robustness of the service API: every dispatch command gets malformed,
//! missing, wrongly typed, huge and hostile arguments. The service must answer
//! with an error (or a harmless result) – it must never panic or hang, and it
//! must keep working afterwards.

use std::sync::Arc;
use std::time::Duration;

use mailcore::host::NullHost;
use mailcore::model::*;
use mailcore::tokens::MemoryTokenStore;
use mailcore::{Service, ServiceOptions};
use serde_json::{json, Map, Value};

/// All commands of `Service::dispatch`, read from its source so new commands
/// are covered automatically.
fn dispatch_commands() -> Vec<String> {
    let src = include_str!("../src/service/mod.rs");
    let start = src.find("pub async fn dispatch").expect("dispatch fn");
    let end = start + src[start..].find("\n    fn open_url").expect("end of dispatch");
    let mut cmds: Vec<String> = src[start..end]
        .lines()
        .filter_map(|l| {
            let l = l.trim_start();
            let rest = l.strip_prefix('"')?;
            let (name, tail) = rest.split_once('"')?;
            tail.trim_start().starts_with("=>").then(|| name.to_string())
        })
        .collect();
    cmds.sort();
    cmds.dedup();
    cmds
}

/// Commands that may reach out to real mail servers / DNS when given a
/// plausible address. They only get arguments that fail validation.
const NETWORK_COMMANDS: &[&str] = &["discover_imap", "test_imap_account", "add_imap_account", "update_imap_account"];

/// Every argument name used anywhere in the API.
const PARAMS: &[&str] = &[
    "accountId", "action", "attachmentId", "autoReply", "bodyHtml", "calendarId", "calendarIds", "categories",
    "category", "color", "comment", "config", "destFolderId", "displayName", "draft", "draftId", "due", "email",
    "end", "eventId", "field", "flag", "flaggedOnly", "folderId", "hasAttachments", "id", "ids", "input",
    "isRead", "kind", "lane", "lanes", "limit", "mailboxId", "messageId", "minutes", "mode", "name", "offset",
    "params", "parentId", "permanent", "query", "recurrence", "response", "sendResponse", "settings", "sortBy",
    "sortDesc", "source", "sourceMessageId", "start", "subject", "text", "unreadOnly", "until", "url",
];

/// Values a hostile or buggy caller might send for an id or a text.
fn nasty_strings() -> Vec<String> {
    vec![
        String::new(),
        " ".into(),
        "..".into(),
        "../../etc/passwd".into(),
        "imap:".into(),
        "imap:nobody@example.invalid".into(),
        "1:2:".into(),
        "4294967296:1:INBOX".into(),
        "%00\0nul".into(),
        "'; DROP TABLE messages; --".into(),
        "%_\\".into(),
        "\u{202E}\u{FEFF}😀ß".into(),
        "javascript:alert(1)".into(),
        "x".repeat(200_000),
    ]
}

/// One argument object with a plausibly typed but hostile value per parameter.
fn typed_args(s: &str) -> Value {
    let mut m = Map::new();
    for p in PARAMS {
        let v = match *p {
            "ids" | "categories" | "lanes" | "calendarIds" => json!([s, s]),
            "isRead" | "permanent" | "hasAttachments" | "sendResponse" | "sortDesc" | "unreadOnly" | "flaggedOnly" => json!(true),
            "minutes" | "offset" | "limit" => json!(i64::MAX),
            "params" => json!({"a": s, "b": 1, "c": null}),
            "config" | "input" | "draft" | "autoReply" | "settings" => json!({"mailboxId": s, "subject": s, "start": s, "end": s}),
            _ => json!(s),
        };
        m.insert(p.to_string(), v);
    }
    Value::Object(m)
}

/// Argument shapes that are wrong as a whole.
fn shapeless_args() -> Vec<Value> {
    let mut v = vec![
        json!({}),
        Value::Null,
        json!([]),
        json!("text"),
        json!(42),
        json!(true),
    ];
    // every parameter with the wrong type
    for wrong in [json!(null), json!(-1), json!(1.5e308), json!([[[]]]), json!({"$ne": 1}), json!(false)] {
        let mut m = Map::new();
        for p in PARAMS {
            m.insert(p.to_string(), wrong.clone());
        }
        v.push(Value::Object(m));
    }
    // deeply nested JSON
    let mut deep = json!("x");
    for _ in 0..64 {
        deep = json!([deep]);
    }
    v.push(json!({"mailboxId": deep, "ids": deep, "input": deep}));
    v
}

async fn setup() -> (Arc<Service>, graph_fake::FakeServer, tempfile::TempDir) {
    let fake = graph_fake::spawn(graph_fake::FakeOptions {
        seed_demo: true,
        now: chrono::Utc::now(),
    })
    .await;
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
    (svc, fake, dir)
}

/// Runs one call on its own task; returns a failure description on panic or hang.
async fn probe(svc: &Arc<Service>, cmd: &str, args: Value) -> Option<String> {
    let svc = svc.clone();
    let c = cmd.to_string();
    let mut shown = args.to_string();
    shown.truncate(shown.char_indices().nth(160).map_or(shown.len(), |(i, _)| i));
    let task = tokio::spawn(async move { svc.dispatch(&c, args).await });
    match tokio::time::timeout(Duration::from_secs(30), task).await {
        Err(_) => Some(format!("{cmd}({shown}): keine Antwort nach 30 s")),
        Ok(Err(e)) if e.is_panic() => Some(format!("{cmd}({shown}): PANIC")),
        Ok(Err(e)) => Some(format!("{cmd}({shown}): {e}")),
        Ok(Ok(_)) => None,
    }
}

#[test]
fn command_list_is_complete() {
    let cmds = dispatch_commands();
    for c in ["get_config", "list_messages", "download_attachment", "send_draft", "open_url", "open_window", "set_focus"] {
        assert!(cmds.iter().any(|x| x == c), "{c} fehlt in {cmds:?}");
    }
    assert!(cmds.len() >= 60, "nur {} Befehle gefunden", cmds.len());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_command_survives_malformed_arguments() {
    // Panics are expected to be caught here, keep the output readable.
    let (svc, _fake, _dir) = setup().await;
    let cmds = dispatch_commands();
    let mut failures = Vec::new();

    for cmd in &cmds {
        for args in shapeless_args() {
            if let Some(f) = probe(&svc, cmd, args).await {
                failures.push(f);
            }
        }
        if NETWORK_COMMANDS.contains(&cmd.as_str()) {
            continue;
        }
        for s in nasty_strings() {
            if let Some(f) = probe(&svc, cmd, typed_args(&s)).await {
                failures.push(f);
            }
        }
    }
    // unknown / hostile command names
    for cmd in ["", "GET_CONFIG", "get_config ", "../get_config", "\0", &"a".repeat(100_000)] {
        match svc.dispatch(cmd, json!({})).await {
            Ok(v) => failures.push(format!("unbekannter Befehl {:?} lieferte {v}", cmd.chars().take(20).collect::<String>())),
            Err(e) => assert!(e.to_string().contains("Unbekannter Befehl"), "{e}"),
        }
    }
    assert!(failures.is_empty(), "{} Probleme:\n{}", failures.len(), failures.join("\n"));

    // ...and the service still works afterwards (no poisoned locks, intact cache)
    let accounts: Vec<Account> = serde_json::from_value(svc.dispatch("list_accounts", json!({})).await.unwrap()).unwrap();
    assert!(!accounts.is_empty());
    let mb = accounts[0].mailboxes[0].id.clone();
    let folders: Vec<Folder> = serde_json::from_value(svc.dispatch("list_folders", json!({"mailboxId": mb})).await.unwrap()).unwrap();
    let inbox = folders.iter().find(|f| f.well_known.as_deref() == Some("inbox")).unwrap();
    let page: MessagePage = serde_json::from_value(
        svc.dispatch("list_messages", json!({"mailboxId": mb, "folderId": inbox.id})).await.unwrap(),
    )
    .unwrap();
    assert!(page.total > 0);
    svc.dispatch("get_config", json!({})).await.unwrap();
}

#[tokio::test]
async fn invalid_input_gives_clear_errors() {
    let (svc, _fake, _dir) = setup().await;
    let err = |r: mailcore::Result<Value>| r.expect_err("Fehler erwartet").to_string();
    let e = err(svc.dispatch("get_message", json!({})).await);
    assert!(e.contains("mailboxId"), "{e}");
    let e = err(svc.dispatch("set_read", json!({"mailboxId": "x", "ids": "nicht-liste", "isRead": true})).await);
    assert!(e.contains("ids"), "{e}");
    let e = err(svc.dispatch("open_window", json!({"kind": "../../etc/passwd"})).await);
    assert!(e.contains("Fenstertyp"), "{e}");
    for url in ["file:///etc/passwd", "javascript:alert(1)", "data:text/html,x", "\\\\server\\share\\x.exe", "C:\\Windows\\calc.exe", "ms-msdt:/id", "search-ms:query=x"] {
        let e = err(svc.dispatch("open_url", json!({"url": url})).await);
        assert!(e.contains("http"), "{url}: {e}");
    }
}

#[tokio::test]
async fn full_resync_after_expired_delta_drops_deleted_mails() {
    let (svc, fake, _dir) = setup().await;
    let accounts: Vec<Account> = serde_json::from_value(svc.dispatch("list_accounts", json!({})).await.unwrap()).unwrap();
    let mb = accounts[0].mailboxes[0].id.clone();
    let me = accounts[0].mailboxes[0].address.clone();
    let folders: Vec<Folder> = serde_json::from_value(svc.dispatch("list_folders", json!({"mailboxId": mb})).await.unwrap()).unwrap();
    let inbox = folders.iter().find(|f| f.well_known.as_deref() == Some("inbox")).unwrap().id.clone();
    let list = || async {
        let p: MessagePage = serde_json::from_value(
            svc.dispatch("list_messages", json!({"mailboxId": mb, "folderId": inbox, "limit": 1000})).await.unwrap(),
        )
        .unwrap();
        p.items.into_iter().map(|m| m.id).collect::<Vec<_>>()
    };
    svc.dispatch("sync_folder", json!({"mailboxId": mb, "folderId": inbox})).await.unwrap();
    let before = list().await;
    let victim = before[0].clone();
    // Deleted elsewhere while the delta token expires (410 syncStateNotFound).
    assert!(fake.state.delete_message(&me, &victim));
    fake.state.fail_next(410, 1);
    svc.dispatch("sync_folder", json!({"mailboxId": mb, "folderId": inbox})).await.unwrap();
    let after = list().await;
    assert!(!after.contains(&victim), "gelöschte Mail bleibt nach Vollabgleich im Cache");
    assert_eq!(after.len(), before.len() - 1, "alle anderen Mails bleiben erhalten");
}
