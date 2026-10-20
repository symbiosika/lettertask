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
        let stamp = chrono::Utc::now().format("%H%M%S").to_string();
        let raw = Raw {
            settings: mailcore::imap::ServerSettings { host: host.clone(), port, security: security(port).into(), username: user.clone() },
            password: password.clone(),
        };
        write_checks(&mut c, &mb, &account.email, &stamp).await;
        edge_checks(&mut c, &mb, &account.email, &stamp, &raw, &input).await;
        println!("\n== Sicherheitsnetz: Reste mit Kennung {stamp} direkt per IMAP entfernen");
        match raw.purge(&stamp).await {
            Ok((msgs, folders)) => c.check(&format!("Server sauber ({msgs} Nachrichten, {folders} Ordner nachträglich entfernt)"), true, ""),
            Err(e) => c.check("Server sauber", false, e.to_string()),
        }
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

async fn write_checks(c: &mut Checker, mb: &str, me: &str, stamp: &str) {
    println!("\n== Schreibtests (alles wird wieder entfernt)");
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

/// A second, independent IMAP connection – plays "another mail client" and
/// cleans up whatever the service layer left behind.
struct Raw {
    settings: mailcore::imap::ServerSettings,
    password: String,
}

impl Raw {
    async fn conn(&self) -> mailcore::Result<mailcore::imap::conn::Conn> {
        mailcore::imap::conn::connect(&self.settings, &self.password).await
    }

    /// UIDs in `folder` whose subject contains `needle`.
    async fn search(&self, folder: &str, needle: &str) -> anyhow::Result<Vec<u32>> {
        let mut c = self.conn().await?;
        c.session.select(folder).await?;
        let mut uids: Vec<u32> = c.session.uid_search(format!("SUBJECT {}", mailcore::imap::quote(needle))).await?.into_iter().collect();
        uids.sort_unstable();
        let _ = c.session.logout().await;
        Ok(uids)
    }

    async fn store(&self, folder: &str, uid: u32, query: &str) -> anyhow::Result<()> {
        use futures::TryStreamExt;
        let mut c = self.conn().await?;
        c.session.select(folder).await?;
        let _: Vec<_> = c.session.uid_store(uid.to_string(), query).await?.try_collect().await?;
        if query.contains("\\Deleted") {
            let _: Vec<_> = c.session.expunge().await?.try_collect().await?;
        }
        let _ = c.session.logout().await;
        Ok(())
    }

    /// Raw source of the first message in `folder` whose subject contains `needle`.
    async fn source(&self, folder: &str, needle: &str) -> anyhow::Result<Option<String>> {
        use futures::TryStreamExt;
        let Some(uid) = self.search(folder, needle).await?.pop() else { return Ok(None) };
        let mut c = self.conn().await?;
        c.session.select(folder).await?;
        let f: Vec<_> = c.session.uid_fetch(uid.to_string(), "BODY.PEEK[]").await?.try_collect().await?;
        let raw = f.first().and_then(|f| f.body()).map(|b| String::from_utf8_lossy(b).into_owned());
        drop(f);
        let _ = c.session.logout().await;
        Ok(raw)
    }

    /// Deletes every message whose subject contains `stamp` and every folder
    /// whose name contains it. Returns (messages, folders) removed.
    async fn purge(&self, stamp: &str) -> anyhow::Result<(usize, usize)> {
        use futures::TryStreamExt;
        let mut c = self.conn().await?;
        let names: Vec<_> = c.session.list(Some(""), Some("*")).await?.try_collect().await?;
        let mut names: Vec<String> = names.iter().filter(|n| !n.attributes().iter().any(|a| format!("{a:?}") == "NoSelect")).map(|n| unescape(n.name())).collect();
        names.sort_by_key(|n| std::cmp::Reverse(n.len()));
        let (mut msgs, mut folders) = (0, 0);
        for name in &names {
            if c.session.select(name).await.is_err() {
                continue;
            }
            let uids: Vec<u32> = c.session.uid_search(format!("SUBJECT {}", mailcore::imap::quote(stamp))).await?.into_iter().collect();
            if !uids.is_empty() {
                msgs += uids.len();
                let _: Vec<_> = c.session.uid_store(mailcore::imap::uid_set(&uids), "+FLAGS.SILENT (\\Deleted)").await?.try_collect().await?;
                let _: Vec<_> = c.session.expunge().await?.try_collect().await?;
            }
        }
        let _ = c.session.close().await;
        for name in names.iter().filter(|n| n.contains(stamp)) {
            if c.session.delete(name).await.is_ok() {
                folders += 1;
            }
        }
        let _ = c.session.logout().await;
        Ok((msgs, folders))
    }
}

/// LIST may hand out quoted-string names with their escapes still in place.
fn unescape(name: &str) -> String {
    name.replace("\\\"", "\"").replace("\\\\", "\\")
}

/// Deterministic pseudo-random bytes (no rand dependency needed).
fn noise(len: usize) -> Vec<u8> {
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 24) as u8
        })
        .collect()
}

async fn wait_for(c: &mut Checker, mb: &str, folder: &str, subject: &str) -> Option<MessageSummary> {
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(90) {
        if let Some(m) = find(c, mb, folder, subject).await {
            return Some(m);
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
    None
}

fn draft(mb: &str, to: Value, subject: &str, body: &str, atts: Value) -> Value {
    json!({"id": null, "mailboxId": mb, "mode": "new", "to": to, "cc": [], "bcc": [], "subject": subject,
        "bodyHtml": body, "importance": "normal", "requestReadReceipt": false,
        "newAttachments": atts, "removedAttachmentIds": []})
}

async fn edge_checks(c: &mut Checker, mb: &str, me: &str, stamp: &str, raw: &Raw, input: &Value) {
    let b64 = |s: &[u8]| base64::engine::general_purpose::STANDARD.encode(s);
    let Some(inbox) = folder(c, mb, "inbox").await else { return };
    let inbox_raw = inbox.id.clone();

    println!("\n== Randfälle: Anmeldung");
    let mut bad = input.clone();
    bad["password"] = json!("falsches-passwort-123");
    bad["smtpPassword"] = json!("falsches-passwort-123");
    let r: Option<Value> = c.call("Verbindungstest mit falschem Passwort", "test_imap_account", json!({"input": bad})).await;
    if let Some(r) = r {
        c.check("IMAP lehnt ab, mit Fehlertext", r["imapOk"] == false && r["imapError"].as_str().is_some_and(|e| !e.is_empty()), r.to_string());
        c.check("SMTP lehnt ab, mit Fehlertext", r["smtpOk"] == false && r["smtpError"].as_str().is_some_and(|e| !e.is_empty()), r.to_string());
        c.check("Fehlertext enthält kein Passwort", !r.to_string().contains("falsches-passwort-123"), "Passwort im Fehlertext");
    }
    let mut unknown = input.clone();
    unknown["imap"]["host"] = json!("imap.unbekannt.invalid");
    unknown["smtp"]["host"] = json!("smtp.unbekannt.invalid");
    let t = Instant::now();
    let r: Option<Value> = c.call("Verbindungstest mit unbekanntem Server", "test_imap_account", json!({"input": unknown})).await;
    if let Some(r) = r {
        c.check("unbekannter Server wird gemeldet", r["imapOk"] == false && r["smtpOk"] == false, r.to_string());
        c.check("Antwort in unter 30 s", t.elapsed() < Duration::from_secs(30), format!("{} s", t.elapsed().as_secs()));
    }

    println!("\n== Randfälle: Ordner mit Sonderzeichen");
    let parent_name = format!("Prüfung Ä ö ü ß & Co {stamp}");
    let parent: Option<Folder> = c.call("Ordner mit Umlauten und & anlegen", "create_folder", json!({"mailboxId": mb, "parentId": null, "name": parent_name})).await;
    if let Some(p) = &parent {
        c.check("Anzeigename exakt erhalten", p.display_name == parent_name, format!("{:?}", p.display_name));
        let child: Option<Folder> = c.call("Unterordner mit € und Emoji anlegen", "create_folder", json!({"mailboxId": mb, "parentId": p.id, "name": "Unter € 📁"})).await;
        if let Some(ch) = &child {
            c.check("Unterordner hängt am Elternordner", ch.parent_id.as_deref() == Some(p.id.as_str()), format!("{:?}", ch.parent_id));
            let renamed: Option<Folder> = c.call("Unterordner umbenennen (Anführungszeichen)", "rename_folder", json!({"mailboxId": mb, "folderId": ch.id, "name": "Kunde \"Müller\""})).await;
            if let Some(r) = &renamed {
                c.check("Neuer Name exakt erhalten", r.display_name == "Kunde \"Müller\"", format!("{:?}", r.display_name));
                let listed: Vec<Folder> = c.call("Ordnerliste nach Umbenennen", "list_folders", json!({"mailboxId": mb})).await.unwrap_or_default();
                c.check("Umbenannter Ordner in der Liste", listed.iter().any(|f| f.id == r.id && f.display_name == r.display_name), "fehlt");
            }
        }
        let dup: Option<Folder> = c.svc.dispatch("create_folder", json!({"mailboxId": mb, "parentId": null, "name": parent_name})).await.ok().and_then(|v| serde_json::from_value(v).ok());
        c.check("Doppelter Ordnername wird abgelehnt oder ist derselbe", dup.as_ref().is_none_or(|d| d.id == p.id), format!("{dup:?}"));
    }
    let err = c.svc.dispatch("delete_folder", json!({"mailboxId": mb, "folderId": inbox.id})).await;
    c.check("Posteingang lässt sich nicht löschen", err.is_err(), "gelöscht?!");
    let err = c.svc.dispatch("create_folder", json!({"mailboxId": mb, "parentId": null, "name": "  "})).await;
    c.check("Leerer Ordnername wird abgelehnt", err.is_err(), "angelegt");

    println!("\n== Randfälle: Versand und Empfang");
    // Header injection: a CR/LF in the subject must not create a new header.
    let inj_subj = format!("{MARK} Injektion {stamp}\r\nX-Injiziert: ja");
    let sent: Option<Value> = c.call("Betreff mit Zeilenumbruch senden", "send_draft", json!({"draft": draft(mb, json!([{"name": "", "address": me}]), &inj_subj, "<p>x</p>", json!([]))})).await;
    if sent.is_some() {
        // The subject may arrive with the line break folded or removed, so
        // wait on the raw server for the stamp instead of an exact subject.
        let mut src = None;
        let t = Instant::now();
        while src.is_none() && t.elapsed() < Duration::from_secs(90) {
            src = raw.source(&inbox_raw, &format!("Injektion {stamp}")).await.ok().flatten();
            if src.is_none() {
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        }
        match src {
            Some(src) => {
                let head = src.split("\r\n\r\n").next().unwrap_or("");
                c.check("Kein injizierter Header", !head.lines().any(|l| l.to_ascii_lowercase().starts_with("x-injiziert")), "X-Injiziert als eigener Header angekommen");
            }
            None => c.check("Mail mit Zeilenumbruch kommt an", false, "nicht gefunden"),
        }
    } else {
        c.note("(Ablehnen des Betreffs ist ebenfalls sicher)");
    }
    // Unicode subject, quoted display name, duplicate recipient, attachments.
    let uni_subj = format!("{MARK} Ünïcödé 😀 „Zitat“ – {stamp} {}", "lang ".repeat(40).trim_end());
    let big = noise(5 * 1024 * 1024);
    let pdf_name = "Prüfbericht März 2026 (final) – Kopie.pdf";
    let atts = json!([
        {"name": pdf_name, "contentType": "application/pdf", "contentBase64": b64(b"%PDF-1.4 Testinhalt")},
        {"name": "gross.bin", "contentType": "application/octet-stream", "contentBase64": b64(&big)},
        {"name": "leer.txt", "contentType": "text/plain", "contentBase64": ""},
    ]);
    let mut d = draft(mb, json!([{"name": "Müller, Hans \"Test\"", "address": me}]), &uni_subj, "<p>Grüße <b>fett</b> &amp; &lt;kein Tag&gt;</p>", atts);
    d["cc"] = json!([{"name": "", "address": me.to_uppercase()}]);
    let t = Instant::now();
    let sent: Option<Value> = c.call("Unicode-Betreff + 5 MB + Umlaut-Anhang senden", "send_draft", json!({"draft": d})).await;
    if sent.is_some() {
        c.note(format!("Versand {} ms", t.elapsed().as_millis()));
        let got = wait_for(c, mb, &inbox_raw, &uni_subj).await;
        c.check("Unicode-Betreff kommt unverändert an", got.is_some(), "Betreff verändert oder nicht angekommen");
        if let Some(m) = got {
            c.check("Anhang-Kennzeichen gesetzt", m.has_attachments, "hasAttachments false");
            c.check("Anzeigename mit Komma und Anführungszeichen", m.to.first().is_some_and(|a| a.name == "Müller, Hans \"Test\""), format!("{:?}", m.to));
            let det: Option<MessageDetail> = c.call("Unicode-Mail öffnen", "get_message", json!({"mailboxId": mb, "messageId": m.id})).await;
            if let Some(det) = det {
                c.check("HTML-Entities bleiben Text", det.body_html.contains("&lt;kein Tag&gt;"), "Entities verändert");
                c.check("Drei Anhänge", det.attachments.len() == 3, format!("{:?}", det.attachments.iter().map(|a| &a.name).collect::<Vec<_>>()));
                c.check("Umlaut-Dateiname exakt", det.attachments.iter().any(|a| a.name == pdf_name), "Dateiname verändert");
                if let Some(a) = det.attachments.iter().find(|a| a.name == "gross.bin") {
                    let t = Instant::now();
                    let p: Option<Option<String>> = c.call("5-MB-Anhang herunterladen", "download_attachment", json!({"mailboxId": mb, "messageId": m.id, "attachmentId": a.id, "action": "save"})).await;
                    if let Some(Some(path)) = p {
                        let bytes = std::fs::read(&path).unwrap_or_default();
                        c.check(&format!("5-MB-Anhang bitgenau ({} ms)", t.elapsed().as_millis()), bytes == big, format!("{} statt {} Bytes", bytes.len(), big.len()));
                        let _ = std::fs::remove_file(path);
                    }
                }
                if let Some(a) = det.attachments.iter().find(|a| a.name == pdf_name) {
                    let p: Option<Option<String>> = c.call("Umlaut-Anhang speichern", "download_attachment", json!({"mailboxId": mb, "messageId": m.id, "attachmentId": a.id, "action": "save"})).await;
                    if let Some(Some(path)) = p {
                        c.check("Gespeicherter Dateiname ohne Pfadanteile", !Path::new(&path).file_name().unwrap_or_default().to_string_lossy().contains('/'), path.clone());
                        let _ = std::fs::remove_file(path);
                    }
                }
            }

            // Reply chain: the reply carries In-Reply-To of the original.
            let reply: Option<Draft> = c.call("Antwort erzeugen", "create_draft", json!({"mailboxId": mb, "mode": "reply", "sourceMessageId": m.id})).await;
            let orig_mid = raw.source(&inbox_raw, stamp).await.ok().flatten().and_then(|s| {
                s.lines().find(|l| l.to_ascii_lowercase().starts_with("message-id:")).map(|l| l[11..].trim().to_string())
            });
            if let Some(r) = reply {
                let mut rd = serde_json::to_value(&r).unwrap_or_default();
                rd["mailboxId"] = json!(mb);
                rd["subject"] = json!(format!("{MARK} Antwortkette {stamp}"));
                rd["newAttachments"] = json!([]);
                rd["removedAttachmentIds"] = json!([]);
                let s: Option<Value> = c.call("Antwort senden", "send_draft", json!({"draft": rd})).await;
                if s.is_some() && wait_for(c, mb, &inbox_raw, &format!("{MARK} Antwortkette {stamp}")).await.is_some() {
                    let src = raw.source(&inbox_raw, &format!("Antwortkette {stamp}")).await.ok().flatten().unwrap_or_default();
                    let irt = src.lines().find(|l| l.to_ascii_lowercase().starts_with("in-reply-to:")).map(|l| l[12..].trim().to_string());
                    c.check("In-Reply-To verweist auf Original", irt.is_some() && irt == orig_mid, format!("{irt:?} vs {orig_mid:?}"));
                }
            }
        }
    }

    println!("\n== Randfälle: zweiter Client ändert gleichzeitig");
    let task_subj = format!("{MARK} Fremdänderung {stamp}");
    let t: Option<MessageSummary> = c.call("Aufgabe anlegen", "create_task", json!({"mailboxId": mb, "subject": task_subj, "bodyHtml": "", "due": null})).await;
    if t.is_some() {
        if let Some(uid) = raw.search(&inbox_raw, &format!("Fremdänderung {stamp}")).await.ok().and_then(|u| u.into_iter().last()) {
            let _ = raw.store(&inbox_raw, uid, "+FLAGS (\\Seen \\Flagged)").await;
            let m = find(c, mb, &inbox_raw, &task_subj).await;
            c.check("Fremd gesetzte Flags nach Sync sichtbar", m.as_ref().is_some_and(|m| m.is_read && m.flag == "flagged"), format!("{:?}", m.map(|m| (m.is_read, m.flag))));
            let cached = c.list(mb, &inbox_raw).await.into_iter().find(|m| m.subject == task_subj);
            let _ = raw.store(&inbox_raw, uid, "+FLAGS (\\Deleted)").await;
            if let Some(stale) = cached {
                let r = c.svc.dispatch("set_flag", json!({"mailboxId": mb, "ids": [stale.id], "flag": "notFlagged"})).await;
                c.note(format!("Aktion auf fremd gelöschte Mail: {}", match r { Ok(_) => "ignoriert".to_string(), Err(e) => format!("Fehler „{e}“") }));
                c.check("Aktion auf fremd gelöschte Mail ohne Absturz", true, "");
                let r = c.svc.dispatch("get_message", json!({"mailboxId": mb, "messageId": stale.id})).await;
                c.check("Öffnen fremd gelöschter Mail meldet einen Fehler", r.is_err(), "lieferte Inhalt");
            }
            let gone = find(c, mb, &inbox_raw, &task_subj).await;
            c.check("Fremd gelöschte Mail verschwindet nach Sync", gone.is_none(), "noch im Cache");
        } else {
            c.check("Aufgabe per zweitem Client gefunden", false, "nicht gefunden");
        }
    }

    println!("\n== Randfälle: Kategorien und Suche");
    let cat_subj = format!("{MARK} Kategorien {stamp}");
    let t: Option<MessageSummary> = c.call("Aufgabe für Kategorien", "create_task", json!({"mailboxId": mb, "subject": cat_subj, "bodyHtml": "<p>Prüfbericht Größe</p>", "due": null})).await;
    if let Some(t) = t {
        let cats = ["Projekt Ä", "Rot & Blau", "100%", "a\"b"];
        let _: Option<Value> = c.call("Kategorien mit Leerzeichen/Umlauten/Sonderzeichen", "set_categories", json!({"mailboxId": mb, "ids": [t.id], "categories": cats})).await;
        let m = find(c, mb, &inbox_raw, &cat_subj).await;
        let got: Vec<String> = m.map(|m| m.categories).unwrap_or_default();
        for cat in cats {
            c.check(&format!("Kategorie „{cat}“ übersteht Sync"), got.iter().any(|g| g == cat), format!("{got:?}"));
        }
        for q in [stamp.to_string(), "Prüfbericht".to_string(), "\"Kategorien".to_string(), "%_*".to_string()] {
            let r = c.svc.dispatch("search_messages", json!({"mailboxId": mb, "text": q, "folderId": null})).await;
            c.check(&format!("Suche nach {q:?} ohne Fehler"), r.is_ok(), format!("{:?}", r.err()));
        }
        let hits: Vec<MessageSummary> = c.svc.dispatch("search_messages", json!({"mailboxId": mb, "text": format!("Kategorien {stamp}"), "folderId": null})).await.ok().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
        c.check("Suche findet die Aufgabe", hits.iter().any(|h| h.subject == cat_subj), format!("{} Treffer", hits.len()));
    }

    println!("\n== Randfälle: Ordner leeren und löschen");
    if let Some(p) = &parent {
        let ids: Vec<String> = c.list(mb, &inbox_raw).await.into_iter().filter(|m| m.subject.contains(stamp)).map(|m| m.id).collect();
        let _: Option<Value> = c.call("Testmails in Umlaut-Ordner verschieben", "move_messages", json!({"mailboxId": mb, "ids": ids, "destFolderId": p.id})).await;
        let _: Option<Value> = c.call("Ordner als gelesen markieren", "mark_folder_read", json!({"mailboxId": mb, "folderId": p.id})).await;
        let left: Vec<MessageSummary> = c.list(mb, &p.id).await;
        c.check("alle gelesen", left.iter().all(|m| m.is_read), format!("{} ungelesen", left.iter().filter(|m| !m.is_read).count()));
        let _: Option<Value> = c.call("Ordner leeren", "empty_folder", json!({"mailboxId": mb, "folderId": p.id})).await;
        let empty = c.list(mb, &p.id).await.is_empty();
        c.check("Ordner leer", empty, "nicht leer");
        let _: Option<Value> = c.call("Ordner mit Unterordner löschen", "delete_folder", json!({"mailboxId": mb, "folderId": p.id})).await;
    }

    println!("\n== Randfälle: Zeilenumbruch im Anzeigenamen");
    // Same for the display name. Runs last: it used to crash the process.
    let inj_name = format!("{MARK} Name {stamp}");
    // In a task of its own so a panic is reported instead of ending the run.
    let svc = c.svc.clone();
    let args = json!({"draft": draft(mb, json!([{"name": "Evil\r\nX-Injiziert: ja", "address": me}]), &inj_name, "<p>x</p>", json!([]))});
    let r = match tokio::spawn(async move { svc.dispatch("send_draft", args).await }).await {
        Ok(r) => r,
        Err(_) => {
            c.check("Senden mit Zeilenumbruch im Namen ohne Absturz", false, "Panic im Service");
            return;
        }
    };
    if r.is_ok() {
        if wait_for(c, mb, &inbox_raw, &inj_name).await.is_some() {
            let src = raw.source(&inbox_raw, &format!("Name {stamp}")).await.ok().flatten().unwrap_or_default();
            let head = src.split("\r\n\r\n").next().unwrap_or("");
            c.check("Anzeigename: kein injizierter Header", !head.lines().any(|l| l.to_ascii_lowercase().starts_with("x-injiziert")), "X-Injiziert angekommen");
        } else {
            c.check("Mail mit Zeilenumbruch im Namen kommt an", false, "nicht gefunden");
        }
    } else {
        c.check("Anzeigename mit Zeilenumbruch abgelehnt", true, "");
    }
}
