//! Live check of an IMAP/SMTP account with the app's own service layer –
//! the IMAP counterpart of `live-check`.
//!
//! ```text
//! IMAP_TEST_HOST=… IMAP_TEST_IMAP_PORT=993 IMAP_TEST_SMTP=… IMAP_TEST_SMTP_PORT=465 \
//! IMAP_TEST_USERNAME=… IMAP_TEST_PASSWORD=… \
//!   cargo run -p devserver --bin live-check-imap -- [--write]
//! ```
//!
//! The user name is also the mail address. Read-only by default. `--write`
//! additionally creates a test folder, tasks, flags, a draft and sends one
//! mail to the account itself; everything it creates is deleted again
//! (only the special folders the app uses, e.g. Wiedervorlage, may remain).
//! Prints counts only, no mail content.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use base64::Engine;
use mailcore::model::*;
use mailcore::tokens::MemoryTokenStore;
use mailcore::{Host, Service, ServiceOptions};
use serde_json::{json, Value};

const MARK: &str = "[Outlook Classic] IMAP-Test";

struct ConsoleHost;

#[async_trait]
impl Host for ConsoleHost {
    fn emit(&self, _event: &str, _payload: Value) {}
    fn open_url(&self, _url: &str) -> mailcore::Result<()> {
        Ok(())
    }
    fn open_path(&self, _path: &Path) -> mailcore::Result<()> {
        Ok(())
    }
    async fn save_dialog(&self, name: &str) -> Option<PathBuf> {
        Some(std::env::temp_dir().join(format!("live-check-imap-{name}")))
    }
    fn notify(&self, _t: &str, _b: &str) {}
    fn open_window(&self, _k: &str, _q: &str) -> mailcore::Result<()> {
        Ok(())
    }
}

struct Checker {
    svc: Arc<Service>,
    ok: usize,
    failed: Vec<String>,
}

impl Checker {
    async fn call<T: serde::de::DeserializeOwned>(&mut self, label: &str, cmd: &str, args: Value) -> Option<T> {
        let t = Instant::now();
        match self.svc.dispatch(cmd, args).await {
            Ok(v) => match serde_json::from_value::<T>(v) {
                Ok(x) => {
                    self.ok += 1;
                    println!("  ✔ {label:<46} {:>6} ms", t.elapsed().as_millis());
                    Some(x)
                }
                Err(e) => self.fail(label, format!("Antwort nicht lesbar: {e}")),
            },
            Err(e) => self.fail(label, e.to_string()),
        }
    }

    fn check(&mut self, label: &str, ok: bool, detail: impl AsRef<str>) {
        if ok {
            self.ok += 1;
            println!("  ✔ {label}");
        } else {
            self.fail::<()>(label, detail.as_ref().to_string());
        }
    }

    fn fail<T>(&mut self, label: &str, why: String) -> Option<T> {
        println!("  ✘ {label:<46} {why}");
        self.failed.push(format!("{label}: {why}"));
        None
    }

    fn note(&self, text: impl AsRef<str>) {
        println!("      {}", text.as_ref());
    }

    async fn list(&mut self, mb: &str, folder: &str) -> Vec<MessageSummary> {
        let p: Option<MessagePage> = self
            .call("Nachrichtenliste", "list_messages", json!({"mailboxId": mb, "folderId": folder, "sortBy": "received", "sortDesc": true, "limit": 200}))
            .await;
        p.map(|p| p.items).unwrap_or_default()
    }
}

fn env(names: &[&str]) -> anyhow::Result<String> {
    names
        .iter()
        .find_map(|n| std::env::var(n).ok().filter(|v| !v.trim().is_empty()))
        .ok_or_else(|| anyhow::anyhow!("Umgebungsvariable fehlt: {}", names.join(" oder ")))
}

fn security(port: u16) -> &'static str {
    match port {
        143 | 587 | 25 => "starttls",
        _ => "ssl",
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut write = false;
    for a in std::env::args().skip(1) {
        match a.as_str() {
            "--write" => write = true,
            other => anyhow::bail!("Unbekanntes Argument: {other}"),
        }
    }
    let host = env(&["IMAP_TEST_HOST"])?;
    let port: u16 = env(&["IMAP_TEST_IMAP_PORT"])?.trim().parse()?;
    let smtp = env(&["IMAP_TEST_SMTP"])?;
    let smtp_port: u16 = env(&["IMAP_TEST_SMTP_PORT"])?.trim().parse()?;
    let user = env(&["IMAP_TEST_USERNAME"])?;
    let password = env(&["IMAP_TEST_PASSWORD"])?;

    let dir = std::env::temp_dir().join(format!("live-check-imap-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let svc = Service::new(
        ServiceOptions {
            data_dir: dir.clone(),
            graph_base: "http://127.0.0.1:9/v1.0".into(),
            authority: "http://127.0.0.1:9".into(),
            demo: false,
            token_store: Arc::new(MemoryTokenStore::default()),
        },
        Arc::new(ConsoleHost),
    )?;
    let mut c = Checker { svc: svc.clone(), ok: 0, failed: vec![] };

    let input = json!({
        "email": user, "displayName": "IMAP-Test",
        "imap": {"host": host, "port": port, "security": security(port), "username": user},
        "smtp": {"host": smtp, "port": smtp_port, "security": security(smtp_port), "username": user},
        "password": password, "smtpPassword": null
    });

    // ---- account ----
    println!("\n== Konto");
    let disc: Option<Value> = c.call("Servereinstellungen ermitteln (Autoconfig)", "discover_imap", json!({"email": user})).await;
    if let Some(d) = disc {
        c.note(format!("Vorschlag IMAP: {}", d["imap"]["host"].as_str().unwrap_or("–")));
    }
    let r: Option<Value> = c.call("Verbindungstest IMAP + SMTP", "test_imap_account", json!({"input": input})).await;
    if let Some(r) = &r {
        c.check("IMAP-Anmeldung", r["imapOk"] == true, r["imapError"].to_string());
        c.check("SMTP-Anmeldung", r["smtpOk"] == true, r["smtpError"].to_string());
    }
    let t = Instant::now();
    let Some(account) = c.call::<Account>("Konto hinzufügen (Ordner + Posteingang)", "add_imap_account", json!({"input": input})).await else {
        anyhow::bail!("Konto konnte nicht angelegt werden");
    };
    c.note(format!("{} ms bis zum ersten Sync", t.elapsed().as_millis()));
    let mb = account.id.clone();

    // ---- read ----
    println!("\n== E-Mail lesen");
    let folders: Vec<Folder> = c.call("Ordnerliste", "list_folders", json!({"mailboxId": mb})).await.unwrap_or_default();
    let wk: Vec<_> = folders.iter().filter_map(|f| f.well_known.clone()).collect();
    c.note(format!("{} Ordner, Standardordner erkannt: {}", folders.len(), wk.join(", ")));
    for need in ["inbox", "sentitems", "drafts", "deleteditems"] {
        c.check(&format!("Standardordner {need}"), wk.iter().any(|w| w == need), "nicht gefunden");
    }
    let folder = |wk: &str| folders.iter().find(|f| f.well_known.as_deref() == Some(wk)).cloned();
    let Some(inbox) = folder("inbox") else {
        anyhow::bail!("Posteingang nicht gefunden");
    };
    c.note(format!("Posteingang laut Server: {} Elemente, {} ungelesen", inbox.total_count, inbox.unread_count));
    let items = c.list(&mb, &inbox.id).await;
    c.note(format!("{} im Cache", items.len()));
    let r: Option<SyncResult> = c.call("Sync erneut (sollte ~0 ändern)", "sync_folder", json!({"mailboxId": mb, "folderId": inbox.id})).await;
    if let Some(r) = r {
        c.note(format!("neu {}, geändert {}, entfernt {}", r.added, r.updated, r.removed));
    }
    let n: Option<i64> = c.call("Ältere Nachrichten nachladen", "load_older", json!({"mailboxId": mb, "folderId": inbox.id})).await;
    c.note(format!("{} ältere geladen", n.unwrap_or(0)));
    for m in items.iter().take(5) {
        let d: Option<MessageDetail> = c.call("Nachricht öffnen (Body, Anhänge)", "get_message", json!({"mailboxId": mb, "messageId": m.id})).await;
        if let Some(d) = d {
            c.note(format!("Body {} Zeichen, {} Anhänge", d.body_html.len(), d.attachments.len()));
        }
    }
    if let Some(first) = items.first() {
        let _: Option<String> = c.call("MIME-Quelltext", "get_message_source", json!({"mailboxId": mb, "messageId": first.id})).await;
        let word = first.subject.split_whitespace().max_by_key(|w| w.len()).unwrap_or("Test").to_string();
        let hits: Option<Vec<MessageSummary>> = c.call("Serversuche", "search_messages", json!({"mailboxId": mb, "text": word, "folderId": null})).await;
        c.note(format!("{} Treffer", hits.map(|h| h.len()).unwrap_or(0)));
    }
    for f in folders.iter().filter(|f| matches!(f.well_known.as_deref(), Some("sentitems" | "drafts" | "deleteditems"))) {
        let _: Option<SyncResult> = c.call(&format!("Sync {}", f.display_name), "sync_folder", json!({"mailboxId": mb, "folderId": f.id})).await;
    }

    // ---- write ----
    if write {
        write_checks(&mut c, &mb, &account.email).await;
    }

    let _: Option<Value> = c.call("Konto entfernen (lokal)", "remove_account", json!({"accountId": mb})).await;
    let _ = std::fs::remove_dir_all(&dir);
    println!("\n== Ergebnis: {} erfolgreich, {} fehlgeschlagen", c.ok, c.failed.len());
    for f in &c.failed {
        println!("   - {f}");
    }
    if !c.failed.is_empty() {
        std::process::exit(1);
    }
    Ok(())
}

/// Finds folders fresh from the cache (snoozed etc. may appear lazily).
async fn folder(c: &mut Checker, mb: &str, wk: &str) -> Option<Folder> {
    let fs: Vec<Folder> = c.svc.dispatch("list_folders", json!({"mailboxId": mb})).await.ok().and_then(|v| serde_json::from_value(v).ok())?;
    fs.into_iter().find(|f| f.well_known.as_deref() == Some(wk))
}

async fn find(c: &mut Checker, mb: &str, folder: &str, subject: &str) -> Option<MessageSummary> {
    let _ = c.svc.dispatch("sync_folder", json!({"mailboxId": mb, "folderId": folder})).await;
    let p: MessagePage = c
        .svc
        .dispatch("list_messages", json!({"mailboxId": mb, "folderId": folder, "sortBy": "received", "sortDesc": true, "limit": 200}))
        .await
        .ok()
        .and_then(|v| serde_json::from_value(v).ok())?;
    p.items.into_iter().find(|m| m.subject == subject)
}

/// Deletes for good: into the trash first, then from the trash.
async fn purge(c: &mut Checker, mb: &str, label: &str, ids: Vec<String>, subjects: &[String]) {
    if ids.is_empty() {
        return;
    }
    let _: Option<Value> = c.call(&format!("{label} löschen (→ Papierkorb)"), "delete_messages", json!({"mailboxId": mb, "ids": ids})).await;
    let Some(trash) = folder(c, mb, "deleteditems").await else { return };
    let mut in_trash = vec![];
    for s in subjects {
        if let Some(m) = find(c, mb, &trash.id, s).await {
            in_trash.push(m.id);
        }
    }
    if !in_trash.is_empty() {
        let _: Option<Value> = c.call(&format!("{label} endgültig löschen"), "delete_messages", json!({"mailboxId": mb, "ids": in_trash})).await;
    }
}

async fn write_checks(c: &mut Checker, mb: &str, me: &str) {
    println!("\n== Schreibtests (alles wird wieder entfernt)");
    let stamp = chrono::Utc::now().format("%H%M%S").to_string();
    let Some(inbox) = folder(c, mb, "inbox").await else { return };

    // Folders.
    let fname = format!("OutlookClassic-Test-{stamp}");
    let f: Option<Folder> = c.call("Ordner anlegen", "create_folder", json!({"mailboxId": mb, "parentId": null, "name": fname})).await;
    let f = match f {
        Some(f) => c.call::<Folder>("Ordner umbenennen", "rename_folder", json!({"mailboxId": mb, "folderId": f.id, "name": format!("{fname}-x")})).await.or(Some(f)),
        None => None,
    };

    // Task without due → unread in the inbox.
    let task_subj = format!("{MARK} Aufgabe {stamp}");
    let t: Option<MessageSummary> = c
        .call("Aufgabe anlegen (sofort → Posteingang)", "create_task", json!({"mailboxId": mb, "subject": task_subj, "bodyHtml": "<p>Test</p>", "due": null}))
        .await;
    if let Some(t) = &t {
        c.check("Aufgabe liegt ungelesen im Posteingang", t.folder_id == inbox.id && !t.is_read, format!("Ordner {}, gelesen {}", t.folder_id, t.is_read));
        let ids = json!([t.id]);
        let _: Option<Value> = c.call("Als gelesen markieren", "set_read", json!({"mailboxId": mb, "ids": ids, "isRead": true})).await;
        let _: Option<Value> = c.call("Kennzeichnen", "set_flag", json!({"mailboxId": mb, "ids": ids, "flag": "flagged"})).await;
        let _: Option<Value> = c.call("Kategorie setzen (IMAP-Schlagwort)", "set_categories", json!({"mailboxId": mb, "ids": ids, "categories": ["OutlookClassic-Test"]})).await;
        if let Some(m) = find(c, mb, &inbox.id, &task_subj).await {
            c.check(
                "Status nach erneutem Sync erhalten",
                m.is_read && m.flag == "flagged" && m.categories.iter().any(|x| x == "OutlookClassic-Test"),
                format!("gelesen {}, Flag {}, Kategorien {:?}", m.is_read, m.flag, m.categories),
            );
        }
        let _: Option<Value> = c.call("Wiederholung setzen (wöchentlich)", "set_recurrence", json!({"mailboxId": mb, "ids": ids, "recurrence": "weekly"})).await;
    }

    // Task with due → Wiedervorlage, then back.
    let due_subj = format!("{MARK} Wiedervorlage {stamp}");
    let due = (chrono::Utc::now() + chrono::Duration::days(3)).to_rfc3339();
    let t2: Option<MessageSummary> = c.call("Aufgabe mit Fälligkeit (→ Wiedervorlage)", "create_task", json!({"mailboxId": mb, "subject": due_subj, "bodyHtml": "", "due": due})).await;
    let mut t2_id = None;
    if let Some(t2) = &t2 {
        c.check("Fälligkeit gesetzt", t2.due_at.is_some(), "dueAt fehlt");
        let back: Option<Value> = c.call("Wiedervorlage aufheben (→ Posteingang)", "unsnooze_messages", json!({"mailboxId": mb, "ids": [t2.id]})).await;
        if back.is_some() {
            t2_id = find(c, mb, &inbox.id, &due_subj).await.map(|m| m.id);
            c.check("wieder im Posteingang", t2_id.is_some(), "nicht gefunden");
        }
    }
    let _: Option<Value> = c.call("Heute-Ansicht", "list_today", json!({})).await;

    // Move + copy into the test folder.
    let mut cleanup_inbox = vec![];
    if let (Some(f), Some(t)) = (&f, &t) {
        let _: Option<Value> = c.call("Kopieren in Testordner", "copy_messages", json!({"mailboxId": mb, "ids": [t.id], "destFolderId": f.id})).await;
        if let Some(id) = &t2_id {
            let _: Option<Value> = c.call("Verschieben in Testordner", "move_messages", json!({"mailboxId": mb, "ids": [id], "destFolderId": f.id})).await;
        }
        let in_f = c.list(mb, &f.id).await;
        c.check("Testordner enthält 2 Nachrichten", in_f.len() == 2, format!("{} gefunden", in_f.len()));
        cleanup_inbox.push(t.id.clone());
    } else {
        cleanup_inbox.extend(t.iter().map(|t| t.id.clone()));
        cleanup_inbox.extend(t2_id.clone());
    }

    // Draft with attachment, saved twice, discarded.
    let b64 = |s: &[u8]| base64::engine::general_purpose::STANDARD.encode(s);
    let mut d = json!({"id": null, "mailboxId": mb, "mode": "new", "to": [{"name": "", "address": me}],
        "cc": [], "bcc": [], "subject": format!("{MARK} Entwurf {stamp}"), "bodyHtml": "<p>Entwurf</p>",
        "importance": "normal", "requestReadReceipt": false,
        "newAttachments": [{"name": "test.txt", "contentType": "text/plain", "contentBase64": b64(b"hallo")}],
        "removedAttachmentIds": []});
    let saved: Option<Draft> = c.call("Entwurf mit Anhang speichern", "save_draft", json!({"draft": d})).await;
    if let Some(s) = saved {
        d["id"] = json!(s.id);
        d["newAttachments"] = json!([]);
        d["subject"] = json!(format!("{MARK} Entwurf {stamp} (2)"));
        let s2: Option<Draft> = c.call("Entwurf erneut speichern", "save_draft", json!({"draft": d})).await;
        if let Some(drafts) = folder(c, mb, "drafts").await {
            let n = c.list(mb, &drafts.id).await.iter().filter(|m| m.subject.contains(&format!("Entwurf {stamp}"))).count();
            c.check("genau ein Entwurf auf dem Server", n == 1, format!("{n} gefunden"));
        }
        let id = s2.map(|x| x.id).unwrap_or(s.id);
        let _: Option<Value> = c.call("Entwurf verwerfen", "discard_draft", json!({"mailboxId": mb, "draftId": id})).await;
    }

    // Send to myself with attachment, wait for it to arrive.
    let sent_subj = format!("{MARK} Versand {stamp}");
    let mail = json!({"id": null, "mailboxId": mb, "mode": "new", "to": [{"name": "", "address": me}],
        "cc": [], "bcc": [], "subject": sent_subj, "bodyHtml": "<p>Automatischer Versandtest – Umlaute: äöüß</p>",
        "importance": "normal", "requestReadReceipt": false,
        "newAttachments": [{"name": "anlage.txt", "contentType": "text/plain", "contentBase64": b64(b"Inhalt der Anlage")}],
        "removedAttachmentIds": []});
    let sent: Option<Value> = c.call("Mail an mich selbst senden (SMTP)", "send_draft", json!({"draft": mail})).await;
    let mut received = None;
    if sent.is_some() {
        if let Some(sf) = folder(c, mb, "sentitems").await {
            let copy = find(c, mb, &sf.id, &sent_subj).await;
            c.check("Kopie in Gesendete Elemente", copy.is_some(), "nicht gefunden");
        }
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(90) {
            if let Some(m) = find(c, mb, &inbox.id, &sent_subj).await {
                received = Some(m);
                break;
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
        c.check("Mail kommt im Posteingang an", received.is_some(), "nach 90 s nicht da");
        if let Some(m) = &received {
            c.note(format!("nach {} s", t.elapsed().as_secs()));
            let det: Option<MessageDetail> = c.call("Empfangene Mail öffnen", "get_message", json!({"mailboxId": mb, "messageId": m.id})).await;
            if let Some(det) = det {
                c.check("Umlaute korrekt", det.body_html.contains("äöüß"), "Body ohne äöüß");
                match det.attachments.iter().find(|a| a.name == "anlage.txt") {
                    Some(a) => {
                        let p: Option<Option<String>> = c
                            .call("Anhang herunterladen", "download_attachment", json!({"mailboxId": mb, "messageId": m.id, "attachmentId": a.id, "action": "save"}))
                            .await;
                        if let Some(Some(path)) = p {
                            let body = std::fs::read(&path).unwrap_or_default();
                            c.check("Anhang inhaltlich korrekt", body == b"Inhalt der Anlage", format!("{} Bytes", body.len()));
                            let _ = std::fs::remove_file(path);
                        }
                    }
                    None => {
                        c.check("Anhang vorhanden", false, "anlage.txt fehlt");
                    }
                }
            }
            let reply: Option<Draft> = c.call("Antwort-Entwurf erzeugen", "create_draft", json!({"mailboxId": mb, "mode": "reply", "sourceMessageId": m.id})).await;
            if let Some(r) = reply {
                c.check("Betreff mit AW:", r.subject.starts_with("AW:"), r.subject.clone());
                if let Some(id) = r.id {
                    let _: Option<Value> = c.call("Antwort-Entwurf verwerfen", "discard_draft", json!({"mailboxId": mb, "draftId": id})).await;
                }
            }
            let _: Option<Value> = c.call("Archivieren", "archive_messages", json!({"mailboxId": mb, "ids": [m.id]})).await;
        }
    }

    // ---- cleanup ----
    println!("\n== Aufräumen");
    let subjects = vec![task_subj.clone(), due_subj.clone(), sent_subj.clone()];
    purge(c, mb, "Aufgaben im Posteingang", cleanup_inbox, &subjects).await;
    if let Some(sf) = folder(c, mb, "sentitems").await {
        let ids: Vec<String> = find(c, mb, &sf.id, &sent_subj).await.into_iter().map(|m| m.id).collect();
        purge(c, mb, "Gesendete Kopie", ids, &subjects).await;
    }
    if received.is_some() {
        if let Some(af) = folder(c, mb, "archive").await {
            let ids: Vec<String> = find(c, mb, &af.id, &sent_subj).await.into_iter().map(|m| m.id).collect();
            purge(c, mb, "Archivierte Mail", ids, &subjects).await;
        }
        let ids: Vec<String> = find(c, mb, &inbox.id, &sent_subj).await.into_iter().map(|m| m.id).collect();
        purge(c, mb, "Empfangene Mail", ids, &subjects).await;
    }
    if let Some(f) = &f {
        let ids: Vec<String> = c.list(mb, &f.id).await.into_iter().map(|m| m.id).collect();
        purge(c, mb, "Inhalt Testordner", ids, &subjects).await;
        let _: Option<Value> = c.call("Testordner löschen", "delete_folder", json!({"mailboxId": mb, "folderId": f.id})).await;
    }
}
