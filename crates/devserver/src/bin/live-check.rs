//! Live check of the Microsoft Graph connection with the app's own service
//! layer. Login via device code (works from any machine/terminal).
//!
//! ```text
//! cargo run -p devserver --bin live-check -- --client-id <GUID> [--tenant <id|domain>] [--write]
//! cargo run -p devserver --bin live-check -- --demo [--write]     # against the built-in fake
//! ```
//!
//! Read-only by default. `--write` additionally creates and removes test
//! objects (draft with attachment, category, calendar event, flag toggle);
//! everything it creates is deleted again. Prints counts only, no mail content.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use base64::Engine;
use mailcore::model::*;
use mailcore::tokens::MemoryTokenStore;
use mailcore::{Host, Service, ServiceOptions};
use serde_json::{json, Value};

struct ConsoleHost;

#[async_trait]
impl Host for ConsoleHost {
    fn emit(&self, _event: &str, _payload: Value) {}
    fn open_url(&self, url: &str) -> mailcore::Result<()> {
        println!("URL: {url}");
        Ok(())
    }
    fn open_path(&self, _path: &Path) -> mailcore::Result<()> {
        Ok(())
    }
    async fn save_dialog(&self, name: &str) -> Option<PathBuf> {
        Some(std::env::temp_dir().join(format!("live-check-{name}")))
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
                Err(e) => {
                    println!("  ✘ {label:<46} Antwort nicht lesbar: {e}");
                    self.failed.push(format!("{label}: {e}"));
                    None
                }
            },
            Err(e) => {
                println!("  ✘ {label:<46} {e}");
                self.failed.push(format!("{label}: {e}"));
                None
            }
        }
    }

    fn note(&self, text: impl AsRef<str>) {
        println!("      {}", text.as_ref());
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut client_id = std::env::var("OUTLOOK_CLIENT_ID").unwrap_or_default();
    let mut tenant = std::env::var("OUTLOOK_TENANT").unwrap_or_else(|_| "organizations".into());
    let (mut write, mut demo) = (false, false);
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--client-id" => client_id = it.next().unwrap_or_default(),
            "--tenant" => tenant = it.next().unwrap_or_default(),
            "--write" => write = true,
            "--demo" => demo = true,
            other => anyhow::bail!("Unbekanntes Argument: {other}"),
        }
    }

    let fake;
    let (graph_base, authority) = if demo {
        fake = graph_fake::spawn(graph_fake::FakeOptions {
            seed_demo: true,
            now: chrono::Utc::now(),
        })
        .await;
        client_id = "demo-client".into();
        (format!("{}/v1.0", fake.base_url), fake.base_url.clone())
    } else {
        if client_id.is_empty() {
            anyhow::bail!("--client-id fehlt (Anwendungs-ID der App-Registrierung)");
        }
        (
            mailcore::graph::DEFAULT_GRAPH_BASE.to_string(),
            mailcore::auth::DEFAULT_AUTHORITY.to_string(),
        )
    };

    let dir = std::env::temp_dir().join(format!("live-check-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let svc = Service::new(
        ServiceOptions {
            data_dir: dir.clone(),
            graph_base,
            authority,
            demo: false,
            token_store: Arc::new(MemoryTokenStore::default()),
        },
        Arc::new(ConsoleHost),
    )?;
    let mut cfg = svc.config();
    cfg.client_id = client_id;
    cfg.tenant = tenant;
    svc.dispatch("save_config", json!({ "config": cfg })).await?;

    // ---- login ----
    println!("\n== Anmeldung (Device Code)");
    let oauth = svc.oauth_config();
    let dc = mailcore::auth::start_device_code(svc.http_client(), &oauth).await?;
    println!("\n  >>> Bitte https://microsoft.com/devicelogin öffnen und Code {} eingeben <<<\n", dc.user_code);
    let tokens = mailcore::auth::poll_device_code(svc.http_client(), &oauth, &dc).await?;
    let t = Instant::now();
    let account = svc.finish_login(tokens).await?;
    println!("  ✔ angemeldet, Ordner + Posteingang synchronisiert   {:>6} ms", t.elapsed().as_millis());

    let mut c = Checker { svc: svc.clone(), ok: 1, failed: vec![] };
    let acc = account.id.clone();
    let mb = account.mailboxes[0].id.clone();

    // ---- mail ----
    println!("\n== E-Mail");
    let folders: Vec<Folder> = c.call("Ordnerliste", "list_folders", json!({"mailboxId": mb})).await.unwrap_or_default();
    let wk: Vec<_> = folders.iter().filter_map(|f| f.well_known.clone()).collect();
    c.note(format!("{} Ordner, Standardordner erkannt: {}", folders.len(), wk.join(", ")));
    let inbox = folders.iter().find(|f| f.well_known.as_deref() == Some("inbox")).cloned();
    let Some(inbox) = inbox else {
        anyhow::bail!("Posteingang nicht gefunden");
    };
    c.note(format!("Posteingang laut Server: {} Elemente, {} ungelesen", inbox.total_count, inbox.unread_count));
    let page: Option<MessagePage> = c
        .call("Nachrichtenliste (Cache)", "list_messages", json!({"mailboxId": mb, "folderId": inbox.id, "limit": 50}))
        .await;
    let page = page.unwrap_or(MessagePage { items: vec![], total: 0, unread: 0, can_load_older: false });
    c.note(format!("{} im Cache (letzte 60 Tage), weitere ladbar: {}", page.total, page.can_load_older));
    let r: Option<SyncResult> = c.call("Delta-Sync erneut (sollte ~0 ändern)", "sync_folder", json!({"mailboxId": mb, "folderId": inbox.id})).await;
    if let Some(r) = r {
        c.note(format!("neu {}, geändert {}, entfernt {}", r.added, r.updated, r.removed));
    }
    let n: Option<i64> = c.call("Ältere Nachrichten nachladen", "load_older", json!({"mailboxId": mb, "folderId": inbox.id})).await;
    c.note(format!("{} ältere geladen", n.unwrap_or(0)));
    let mut with_att = None;
    for m in page.items.iter().take(5) {
        let d: Option<MessageDetail> = c
            .call("Nachricht öffnen (Body, Anhänge)", "get_message", json!({"mailboxId": mb, "messageId": m.id}))
            .await;
        if let Some(d) = d {
            c.note(format!(
                "Body {} Zeichen, {} Anhänge, Einladung: {}",
                d.body_html.len(),
                d.attachments.len(),
                d.is_meeting_request
            ));
            if with_att.is_none() && !d.attachments.is_empty() {
                with_att = Some((m.id.clone(), d.attachments[0].clone()));
            }
        }
    }
    if let Some((mid, a)) = &with_att {
        let p: Option<Option<String>> = c
            .call(
                "Anhang herunterladen",
                "download_attachment",
                json!({"mailboxId": mb, "messageId": mid, "attachmentId": a.id, "action": "save"}),
            )
            .await;
        if let Some(Some(path)) = p {
            let len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            c.note(format!("{len} Bytes (erwartet {})", a.size));
            let _ = std::fs::remove_file(path);
        }
    }
    if let Some(first) = page.items.first() {
        let word = first.subject.split_whitespace().max_by_key(|w| w.len()).unwrap_or("Test").to_string();
        let hits: Option<Vec<MessageSummary>> = c.call("Serversuche ($search)", "search_messages", json!({"mailboxId": mb, "text": word, "folderId": null})).await;
        c.note(format!("{} Treffer", hits.map(|h| h.len()).unwrap_or(0)));
        let _: Option<String> = c.call("MIME-Quelltext", "get_message_source", json!({"mailboxId": mb, "messageId": first.id})).await;
    }
    let sugg: Option<Vec<Address>> = c.call("Adressvorschläge (People-API)", "suggest_recipients", json!({"mailboxId": mb, "query": account.email.chars().take(3).collect::<String>()})).await;
    c.note(format!("{} Vorschläge", sugg.map(|s| s.len()).unwrap_or(0)));
    for f in folders.iter().filter(|f| matches!(f.well_known.as_deref(), Some("sentitems" | "drafts"))) {
        let _: Option<SyncResult> = c.call(&format!("Sync {}", f.display_name), "sync_folder", json!({"mailboxId": mb, "folderId": f.id})).await;
    }

    // ---- categories, calendar, settings ----
    println!("\n== Kategorien, Kalender, Einstellungen");
    let cats: Option<Vec<Category>> = c.call("Masterkategorien", "list_categories", json!({"accountId": acc})).await;
    c.note(format!("{} Kategorien", cats.map(|x| x.len()).unwrap_or(0)));
    let cals: Vec<Calendar> = c.call("Kalenderliste", "list_calendars", json!({"accountId": acc})).await.unwrap_or_default();
    c.note(format!("{} Kalender, davon {} schreibbar", cals.len(), cals.iter().filter(|x| x.can_edit).count()));
    let now = chrono::Utc::now();
    let ids: Vec<String> = cals.iter().map(|x| x.id.clone()).collect();
    let evs: Option<Vec<Event>> = c
        .call(
            "Termine ±14 Tage (calendarView)",
            "list_events",
            json!({"accountId": acc, "calendarIds": ids, "start": (now - chrono::Duration::days(14)).to_rfc3339(), "end": (now + chrono::Duration::days(14)).to_rfc3339()}),
        )
        .await;
    if let Some(evs) = &evs {
        c.note(format!(
            "{} Termine, {} Serienvorkommen, {} ganztägig",
            evs.len(),
            evs.iter().filter(|e| e.kind == "occurrence").count(),
            evs.iter().filter(|e| e.is_all_day).count()
        ));
        if let Some(e) = evs.first() {
            let _: Option<EventDetail> = c.call("Termin öffnen", "get_event", json!({"accountId": acc, "eventId": e.id})).await;
        }
    }
    let ar: Option<AutoReply> = c.call("Abwesenheitsnotiz lesen", "get_auto_reply", json!({"accountId": acc})).await;
    c.note(format!("Status: {}", ar.map(|a| a.status).unwrap_or_default()));

    // ---- write checks ----
    if write {
        println!("\n== Schreibtests (alles wird wieder entfernt)");
        let draft = json!({"id": null, "mailboxId": mb, "mode": "new", "to": [{"name": "", "address": account.email}],
            "cc": [], "bcc": [], "subject": "[Outlook Classic] Verbindungstest – kann gelöscht werden",
            "bodyHtml": "<p>Automatischer Test.</p>", "importance": "normal", "requestReadReceipt": false,
            "newAttachments": [{"name": "test.txt", "contentType": "text/plain",
                "contentBase64": base64::engine::general_purpose::STANDARD.encode("hallo")}],
            "removedAttachmentIds": []});
        let d: Option<Draft> = c.call("Entwurf mit Anhang speichern", "save_draft", json!({"draft": draft})).await;
        if let Some(d) = d {
            c.note(format!("{} Anhang/Anhänge am Entwurf", d.attachments.len()));
            let _: Option<Value> = c.call("Entwurf verwerfen", "discard_draft", json!({"mailboxId": mb, "draftId": d.id})).await;
        }
        if let Some(m) = page.items.iter().find(|m| m.flag == "notFlagged") {
            let _: Option<Value> = c.call("Kennzeichnen", "set_flag", json!({"mailboxId": mb, "ids": [m.id], "flag": "flagged"})).await;
            let _: Option<Value> = c.call("Kennzeichnung zurücksetzen", "set_flag", json!({"mailboxId": mb, "ids": [m.id], "flag": "notFlagged"})).await;
        }
        let cat: Option<Category> = c
            .call("Kategorie anlegen", "create_category", json!({"accountId": acc, "displayName": "OutlookClassic-Test", "color": "preset9"}))
            .await;
        if let Some(cat) = cat {
            let _: Option<Category> = c.call("Kategoriefarbe ändern", "update_category", json!({"accountId": acc, "id": cat.id, "color": "preset3"})).await;
            let _: Option<Value> = c.call("Kategorie löschen", "delete_category", json!({"accountId": acc, "id": cat.id})).await;
        }
        if let Some(cal) = cals.iter().find(|x| x.is_default && x.can_edit) {
            let day = (now + chrono::Duration::days(30)).format("%Y-%m-%d").to_string();
            let input = json!({"subject": "[Outlook Classic] Verbindungstest", "start": format!("{day}T10:00:00"),
                "end": format!("{day}T10:30:00"), "timeZone": "Europe/Berlin", "isAllDay": false, "location": "",
                "bodyHtml": "", "attendees": [], "showAs": "free", "categories": [], "isReminderOn": false,
                "reminderMinutes": 0, "isOnlineMeeting": false, "recurrence": null, "sensitivity": "normal", "importance": "normal"});
            let ev: Option<Event> = c.call("Termin anlegen (Europe/Berlin)", "create_event", json!({"accountId": acc, "calendarId": cal.id, "input": input})).await;
            if let Some(ev) = ev {
                c.note(format!("Beginn in UTC: {}", ev.start));
                let mut changed = input.clone();
                changed["end"] = json!(format!("{day}T11:00:00"));
                let _: Option<Event> = c.call("Termin ändern", "update_event", json!({"accountId": acc, "eventId": ev.id, "input": changed})).await;
                let _: Option<Value> = c.call("Termin löschen", "delete_event", json!({"accountId": acc, "eventId": ev.id})).await;
            }
        }
    }

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
