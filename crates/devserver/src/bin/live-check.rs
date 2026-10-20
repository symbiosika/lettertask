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

    fn check(&mut self, label: &str, ok: bool, detail: impl AsRef<str>) {
        if ok {
            self.ok += 1;
            println!("  ✔ {label}");
        } else {
            println!("  ✘ {label:<46} {}", detail.as_ref());
            self.failed.push(format!("{label}: {}", detail.as_ref()));
        }
    }

    async fn raw(&self, cmd: &str, args: Value) -> mailcore::Result<Value> {
        self.svc.dispatch(cmd, args).await
    }

    async fn folder(&self, mb: &str, wk: &str) -> Option<Folder> {
        let fs: Vec<Folder> = serde_json::from_value(self.raw("list_folders", json!({"mailboxId": mb})).await.ok()?).ok()?;
        fs.into_iter().find(|f| f.well_known.as_deref() == Some(wk))
    }

    /// Syncs `folder` and returns the messages whose subject contains `needle`.
    async fn find(&self, mb: &str, folder: &str, needle: &str) -> Vec<MessageSummary> {
        let _ = self.raw("sync_folder", json!({"mailboxId": mb, "folderId": folder})).await;
        let p: Option<MessagePage> = self
            .raw("list_messages", json!({"mailboxId": mb, "folderId": folder, "sortBy": "received", "sortDesc": true, "limit": 200}))
            .await
            .ok()
            .and_then(|v| serde_json::from_value(v).ok());
        p.map(|p| p.items).unwrap_or_default().into_iter().filter(|m| m.subject.contains(needle)).collect()
    }

    async fn wait_for(&self, mb: &str, folder: &str, needle: &str) -> Option<MessageSummary> {
        let t = Instant::now();
        while t.elapsed() < std::time::Duration::from_secs(120) {
            if let Some(m) = self.find(mb, folder, needle).await.into_iter().next() {
                return Some(m);
            }
            tokio::time::sleep(std::time::Duration::from_secs(4)).await;
        }
        None
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut client_id = std::env::var("OUTLOOK_CLIENT_ID").unwrap_or_else(|_| mailcore::model::builtin_client_id().to_string());
    let mut tenant = std::env::var("OUTLOOK_TENANT").unwrap_or_else(|_| mailcore::model::builtin_tenant().to_string());
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

    if write {
        edge_checks(&mut c, &acc, &mb, &account.email, &cals).await;
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

const MARK: &str = "[Outlook Classic] Graph-Test";

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

fn draft(mb: &str, to: Value, subject: &str, body: &str, atts: Value) -> Value {
    json!({"id": null, "mailboxId": mb, "mode": "new", "to": to, "cc": [], "bcc": [], "subject": subject,
        "bodyHtml": body, "importance": "normal", "requestReadReceipt": false,
        "newAttachments": atts, "removedAttachmentIds": []})
}

fn event_input(subject: &str, start: &str, end: &str, tz: &str, all_day: bool, recurrence: Value) -> Value {
    json!({"subject": subject, "start": start, "end": end, "timeZone": tz, "isAllDay": all_day, "location": "Raum „Ü“",
        "bodyHtml": "", "attendees": [], "showAs": "free", "categories": [], "isReminderOn": false,
        "reminderMinutes": 0, "isOnlineMeeting": false, "recurrence": recurrence, "sensitivity": "normal", "importance": "normal"})
}

fn utc(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(s).ok().map(|d| d.with_timezone(&chrono::Utc))
}

/// Edge cases against the real service. Everything created carries `stamp`
/// and is removed again at the end.
async fn edge_checks(c: &mut Checker, acc: &str, mb: &str, me: &str, cals: &[Calendar]) {
    let b64 = |s: &[u8]| base64::engine::general_purpose::STANDARD.encode(s);
    let stamp = chrono::Utc::now().format("%H%M%S").to_string();
    let mut events: Vec<String> = vec![];

    // ---- calendar: time zones, DST, all-day, series ----
    if let Some(cal) = cals.iter().find(|x| x.is_default && x.can_edit) {
        println!("\n== Randfälle: Kalender und Zeitzonen");
        let cases = [
            ("Winterzeit", "2026-11-19T10:00:00", "2026-11-19T10:30:00", "Europe/Berlin", "2026-11-19T09:00:00Z", 30),
            ("Sommerzeit", "2027-07-01T10:00:00", "2027-07-01T11:00:00", "Europe/Berlin", "2027-07-01T08:00:00Z", 60),
            ("über die Zeitumstellung", "2027-03-27T22:00:00", "2027-03-28T04:00:00", "Europe/Berlin", "2027-03-27T21:00:00Z", 300),
            ("New York", "2026-12-01T09:00:00", "2026-12-01T09:15:00", "America/New_York", "2026-12-01T14:00:00Z", 15),
            ("UTC", "2026-12-02T09:00:00", "2026-12-02T09:45:00", "UTC", "2026-12-02T09:00:00Z", 45),
        ];
        for (label, start, end, tz, want, minutes) in cases {
            let subj = format!("{MARK} {label} {stamp}");
            let ev: Option<Event> = c
                .call(&format!("Termin {label} anlegen"), "create_event", json!({"accountId": acc, "calendarId": cal.id, "input": event_input(&subj, start, end, tz, false, Value::Null)}))
                .await;
            let Some(ev) = ev else { continue };
            events.push(ev.id.clone());
            let (s, e) = (utc(&ev.start), utc(&ev.end));
            c.check(&format!("{label}: Beginn {want} (Antwort auf Anlegen)"), s == utc(want), &ev.start);
            c.check(&format!("{label}: Dauer {minutes} min"), matches!((s, e), (Some(s), Some(e)) if (e - s).num_minutes() == minutes), format!("{} – {}", ev.start, ev.end));
            let got: Option<EventDetail> = c.call(&format!("{label}: Termin neu laden"), "get_event", json!({"accountId": acc, "eventId": ev.id})).await;
            if let Some(got) = got {
                let gs = serde_json::to_value(&got).ok().and_then(|v| v["start"].as_str().map(str::to_string)).unwrap_or_default();
                c.check(&format!("{label}: Neuladen liefert dieselbe Zeit"), utc(&gs) == s, format!("{gs} vs {}", ev.start));
            }
        }
        // Update across DST: move the winter event into summer.
        if let Some(id) = events.first().cloned() {
            let input = event_input(&format!("{MARK} verschoben {stamp}"), "2027-06-15T10:00:00", "2027-06-15T10:30:00", "Europe/Berlin", false, Value::Null);
            let ev: Option<Event> = c.call("Termin in die Sommerzeit verschieben", "update_event", json!({"accountId": acc, "eventId": id, "input": input})).await;
            if let Some(ev) = ev {
                c.check("Verschoben: Beginn 08:00Z (Antwort auf Ändern)", utc(&ev.start) == utc("2027-06-15T08:00:00Z"), &ev.start);
            }
        }
        // All-day over two days.
        let subj = format!("{MARK} ganztägig {stamp}");
        let ev: Option<Event> = c
            .call("Ganztägigen Termin (2 Tage) anlegen", "create_event", json!({"accountId": acc, "calendarId": cal.id, "input": event_input(&subj, "2026-12-24T00:00:00", "2026-12-26T00:00:00", "Europe/Berlin", true, Value::Null)}))
            .await;
        if let Some(ev) = ev {
            events.push(ev.id.clone());
            c.check("ganztägig markiert", ev.is_all_day, "isAllDay false");
            let in_view: Option<Vec<Event>> = c
                .call("Ganztägig in Ansicht 25.12.", "list_events", json!({"accountId": acc, "calendarIds": [cal.id], "start": "2026-12-25T00:00:00Z", "end": "2026-12-25T23:59:00Z"}))
                .await;
            c.check("erscheint am zweiten Tag", in_view.is_some_and(|v| v.iter().any(|e| e.subject == subj)), "fehlt");
            let next_day: Option<Vec<Event>> = c
                .call("Ganztägig in Ansicht 26.12. (danach)", "list_events", json!({"accountId": acc, "calendarIds": [cal.id], "start": "2026-12-26T12:00:00Z", "end": "2026-12-26T23:00:00Z"}))
                .await;
            c.check("nicht mehr am 26.12. mittags", next_day.is_some_and(|v| !v.iter().any(|e| e.subject == subj)), "ragt in den 26.12.");
        }
        // Weekly series with 5 occurrences across the DST switch in March.
        let subj = format!("{MARK} Serie {stamp}");
        let rec = json!({"pattern": "weekly", "interval": 1, "daysOfWeek": ["thursday"], "dayOfMonth": 0, "month": 0, "startDate": "2027-03-11", "endDate": null, "occurrences": 5});
        let ev: Option<Event> = c
            .call("Wöchentliche Serie (5×, über Zeitumstellung)", "create_event", json!({"accountId": acc, "calendarId": cal.id, "input": event_input(&subj, "2027-03-11T09:00:00", "2027-03-11T09:30:00", "Europe/Berlin", false, rec)}))
            .await;
        if let Some(ev) = ev {
            events.push(ev.id.clone());
            let occ: Vec<Event> = c
                .call::<Vec<Event>>("Serie in Ansicht März/April", "list_events", json!({"accountId": acc, "calendarIds": [cal.id], "start": "2027-03-01T00:00:00Z", "end": "2027-04-30T00:00:00Z"}))
                .await
                .unwrap_or_default()
                .into_iter()
                .filter(|e| e.subject == subj)
                .collect();
            c.check("5 Vorkommen", occ.len() == 5, format!("{} Vorkommen", occ.len()));
            let hours: Vec<String> = occ.iter().filter_map(|e| utc(&e.start)).map(|d| d.format("%H:%M").to_string()).collect();
            c.check(
                "Ortszeit bleibt 09:00 (3× 08:00Z vor, 2× 07:00Z nach der Umstellung)",
                hours.iter().filter(|h| *h == "08:00").count() == 3 && hours.iter().filter(|h| *h == "07:00").count() == 2,
                format!("{hours:?}"),
            );
            if let Some(o) = occ.get(2) {
                let _: Option<Value> = c.call("Einzelnes Vorkommen löschen", "delete_event", json!({"accountId": acc, "eventId": o.id})).await;
                let left = c
                    .raw("list_events", json!({"accountId": acc, "calendarIds": [cal.id], "start": "2027-03-01T00:00:00Z", "end": "2027-04-30T00:00:00Z"}))
                    .await
                    .ok()
                    .and_then(|v| serde_json::from_value::<Vec<Event>>(v).ok())
                    .unwrap_or_default();
                c.check("danach 4 Vorkommen", left.iter().filter(|e| e.subject == subj).count() == 4, format!("{}", left.iter().filter(|e| e.subject == subj).count()));
            }
        }
        let bad = event_input("x", "2026-12-01T10:00:00", "2026-12-01T09:00:00", "Europe/Berlin", false, Value::Null);
        let r = c.raw("create_event", json!({"accountId": acc, "calendarId": cal.id, "input": bad})).await;
        c.check("Ende vor Beginn wird abgelehnt", r.is_err(), "angelegt");
        if let Ok(v) = r {
            if let Some(id) = v["id"].as_str() {
                events.push(id.to_string());
            }
        }
        for id in &events {
            let _: Option<Value> = c.call("Testtermin löschen", "delete_event", json!({"accountId": acc, "eventId": id})).await;
        }
    }

    // ---- mail ----
    println!("\n== Randfälle: Ordner");
    let Some(inbox) = c.folder(mb, "inbox").await else { return };
    let pname = format!("Prüfung Ä ö ü ß & Co {stamp}");
    let parent: Option<Folder> = c.call("Ordner mit Umlauten und & anlegen", "create_folder", json!({"mailboxId": mb, "parentId": null, "name": pname})).await;
    if let Some(p) = &parent {
        c.check("Anzeigename exakt", p.display_name == pname, &p.display_name);
        let ch: Option<Folder> = c.call("Unterordner mit € und Emoji", "create_folder", json!({"mailboxId": mb, "parentId": p.id, "name": "Unter € 📁"})).await;
        if let Some(ch) = ch {
            let r: Option<Folder> = c.call("Unterordner umbenennen (Anführungszeichen)", "rename_folder", json!({"mailboxId": mb, "folderId": ch.id, "name": "Kunde \"Müller\""})).await;
            c.check("Neuer Name exakt", r.is_some_and(|r| r.display_name == "Kunde \"Müller\""), "abweichend");
        }
    }
    c.check("Posteingang lässt sich nicht löschen", c.raw("delete_folder", json!({"mailboxId": mb, "folderId": inbox.id})).await.is_err(), "gelöscht?!");
    c.check("Leerer Ordnername abgelehnt", c.raw("create_folder", json!({"mailboxId": mb, "parentId": null, "name": " "})).await.is_err(), "angelegt");

    println!("\n== Randfälle: Versand und Empfang");
    let subj = format!("{MARK} Ünïcödé 😀 „Zitat“ {stamp} {}", "lang ".repeat(40).trim_end());
    let big = noise(5 * 1024 * 1024);
    let pdf = "Prüfbericht März 2026 (final) – Kopie.pdf";
    let atts = json!([
        {"name": pdf, "contentType": "application/pdf", "contentBase64": b64(b"%PDF-1.4 Testinhalt")},
        {"name": "gross.bin", "contentType": "application/octet-stream", "contentBase64": b64(&big)},
    ]);
    let mut d = draft(mb, json!([{"name": "Müller, Hans \"Test\"", "address": me}]), &subj, "<p>Grüße &amp; &lt;kein Tag&gt;</p>", atts);
    d["cc"] = json!([{"name": "", "address": me.to_uppercase()}]);
    let sent: Option<Value> = c.call("Unicode-Betreff + 5 MB + Umlaut-Anhang senden", "send_draft", json!({"draft": d})).await;
    if sent.is_some() {
        let received = c.wait_for(mb, &inbox.id, &stamp).await;
        c.check("Mail kommt im Posteingang an", received.is_some(), "nach 120 s nicht da");
        if let Some(m) = &received {
            c.check("Betreff unverändert", m.subject == subj, &m.subject);
            // Exchange replaces the name of internal recipients with the one from
            // the address book, so only the address can be checked here.
            c.check("Empfängeradresse erhalten", m.to.first().is_some_and(|a| a.address.eq_ignore_ascii_case(me)), format!("{:?}", m.to));
            c.note(format!("Anzeigename laut Server: {:?}", m.to.first().map(|a| &a.name)));
            let det: Option<MessageDetail> = c.call("Mail öffnen", "get_message", json!({"mailboxId": mb, "messageId": m.id})).await;
            if let Some(det) = det {
                c.check("Entities bleiben Text", det.body_html.contains("&lt;kein Tag&gt;"), "verändert");
                c.check("Umlaut-Dateiname exakt", det.attachments.iter().any(|a| a.name == pdf), format!("{:?}", det.attachments.iter().map(|a| &a.name).collect::<Vec<_>>()));
                if let Some(a) = det.attachments.iter().find(|a| a.name == "gross.bin") {
                    let p: Option<Option<String>> = c.call("5-MB-Anhang herunterladen", "download_attachment", json!({"mailboxId": mb, "messageId": m.id, "attachmentId": a.id, "action": "save"})).await;
                    if let Some(Some(path)) = p {
                        let bytes = std::fs::read(&path).unwrap_or_default();
                        c.check("5-MB-Anhang bitgenau", bytes == big, format!("{} statt {} Bytes", bytes.len(), big.len()));
                        let _ = std::fs::remove_file(path);
                    }
                } else {
                    c.check("5-MB-Anhang vorhanden", false, "fehlt");
                }
            }
        }
    }
    // Reply chain with a short subject: Exchange derives the conversation from
    // the subject, and very long subjects are cut differently in replies.
    let chain = format!("{MARK} Kette {stamp}");
    let sent: Option<Value> = c.call("Mail für Antwortkette senden", "send_draft", json!({"draft": draft(mb, json!([{"name": "", "address": me}]), &chain, "<p>x</p>", json!([]))})).await;
    if sent.is_some() {
        if let Some(m) = c.wait_for(mb, &inbox.id, &chain).await {
            let reply: Option<Draft> = c.call("Antwort erzeugen", "create_draft", json!({"mailboxId": mb, "mode": "reply", "sourceMessageId": m.id})).await;
            if let Some(r) = reply {
                c.check("Betreff mit AW:/RE:", r.subject.starts_with("AW:") || r.subject.starts_with("RE:"), &r.subject);
                c.check("Antwort geht an den Absender", r.to.iter().any(|a| a.address.eq_ignore_ascii_case(me)), format!("{:?}", r.to));
                let mut rd = serde_json::to_value(&r).unwrap_or_default();
                rd["mailboxId"] = json!(mb);
                rd["newAttachments"] = json!([]);
                rd["removedAttachmentIds"] = json!([]);
                let s: Option<Value> = c.call("Antwort senden", "send_draft", json!({"draft": rd})).await;
                if s.is_some() {
                    let reply_subject = r.subject.clone();
                    let t = Instant::now();
                    let mut got = None;
                    while got.is_none() && t.elapsed() < std::time::Duration::from_secs(120) {
                        got = c.find(mb, &inbox.id, &stamp).await.into_iter().find(|x| x.subject == reply_subject);
                        if got.is_none() {
                            tokio::time::sleep(std::time::Duration::from_secs(4)).await;
                        }
                    }
                    match got {
                        Some(g) => c.check("Antwort in derselben Unterhaltung", g.conversation_id == m.conversation_id, format!("{} vs {}", g.conversation_id, m.conversation_id)),
                        None => {
                            let seen: Vec<String> = c.find(mb, &inbox.id, &stamp).await.into_iter().map(|x| x.subject).collect();
                            c.check("Antwort kommt an", false, format!("erwartet {reply_subject:?}, im Posteingang: {seen:?}"));
                        }
                    }
                }
            }
        } else {
            c.check("Mail für Antwortkette kommt an", false, "nicht gefunden");
        }
    }
    let inj = format!("{MARK} Injektion {stamp}\r\nX-Injiziert: ja");
    let svc = c.svc.clone();
    let args = json!({"draft": draft(mb, json!([{"name": "Evil\r\nX-Injiziert: ja", "address": me}]), &inj, "<p>x</p>", json!([]))});
    match tokio::spawn(async move { svc.dispatch("send_draft", args).await }).await {
        Err(_) => c.check("Zeilenumbruch in Betreff/Name ohne Absturz", false, "Panic im Service"),
        Ok(Err(e)) => c.check(&format!("Zeilenumbruch in Betreff/Name abgelehnt ({e})"), true, ""),
        Ok(Ok(_)) => {
            let m = c.wait_for(mb, &inbox.id, &format!("Injektion {stamp}")).await;
            match m {
                Some(m) => {
                    let src: Option<String> = c.call("Quelltext der Injektions-Mail", "get_message_source", json!({"mailboxId": mb, "messageId": m.id})).await;
                    let src = src.unwrap_or_default();
                    let head = src.split("\r\n\r\n").next().unwrap_or("");
                    c.check("Kein injizierter Header", !head.lines().any(|l| l.to_ascii_lowercase().starts_with("x-injiziert")), "X-Injiziert angekommen");
                }
                None => c.check("Injektions-Mail kommt an", false, "nicht gefunden"),
            }
        }
    }

    println!("\n== Randfälle: Aufgaben, Wiedervorlage, Kategorien, Suche");
    let tsubj = format!("{MARK} Aufgabe {stamp}");
    let t: Option<MessageSummary> = c.call("Aufgabe anlegen", "create_task", json!({"mailboxId": mb, "subject": tsubj, "bodyHtml": "<p>Prüfbericht</p>", "due": null})).await;
    if let Some(t) = t {
        c.check("Aufgabe ungelesen im Posteingang", t.folder_id == inbox.id && !t.is_read, format!("{} {}", t.folder_id, t.is_read));
        let cats = ["Projekt Ä", "Rot & Blau", "100%", "a\"b"];
        let _: Option<Value> = c.call("Kategorien mit Sonderzeichen setzen", "set_categories", json!({"mailboxId": mb, "ids": [t.id], "categories": cats})).await;
        let got = c.find(mb, &inbox.id, &tsubj).await.into_iter().next().map(|m| m.categories).unwrap_or_default();
        for cat in cats {
            c.check(&format!("Kategorie „{cat}“ übersteht Sync"), got.iter().any(|g| g == cat), format!("{got:?}"));
        }
        let until = (chrono::Utc::now() + chrono::Duration::days(2)).to_rfc3339();
        let _: Option<Value> = c.call("Zurückstellen (2 Tage)", "snooze_messages", json!({"mailboxId": mb, "ids": [t.id], "until": until})).await;
        c.check("nicht mehr im Posteingang", c.find(mb, &inbox.id, &tsubj).await.is_empty(), "noch da");
        if let Some(sn) = c.folder(mb, "snoozed").await {
            if let Some(m) = c.find(mb, &sn.id, &tsubj).await.into_iter().next() {
                c.check("in Wiedervorlage mit Fälligkeit", m.due_at.is_some(), "dueAt fehlt");
                let _: Option<Value> = c.call("Wiedervorlage aufheben", "unsnooze_messages", json!({"mailboxId": mb, "ids": [m.id]})).await;
                c.check("zurück im Posteingang", !c.find(mb, &inbox.id, &tsubj).await.is_empty(), "fehlt");
            } else {
                c.check("in Wiedervorlage gefunden", false, "fehlt");
            }
        }
    }
    for q in [stamp.clone(), "Prüfbericht".into(), "\"Aufgabe".into(), "a OR b".into(), "subject:(x".into(), "%_*\\".into()] {
        let r = c.raw("search_messages", json!({"mailboxId": mb, "text": q, "folderId": null})).await;
        c.check(&format!("Suche {q:?} ohne Fehler"), r.is_ok(), format!("{:?}", r.err()));
    }

    println!("\n== Randfälle: Kontakte, Abwesenheit, ungültige Kennungen");
    let ct: Option<Value> = c
        .call("Kontakt mit Umlauten anlegen", "create_contact", json!({"accountId": acc, "input": {"givenName": "Jürgen", "surname": format!("Ößterreicher {stamp}"), "emails": ["juergen.test@example.invalid"], "company": "Müller & Söhne"}}))
        .await;
    if let Some(id) = ct.as_ref().and_then(|v| v["id"].as_str()) {
        let u: Option<Value> = c.call("Kontakt ändern", "update_contact", json!({"id": id, "input": {"givenName": "Jürgen", "surname": format!("Ößterreicher {stamp}"), "emails": ["juergen.test@example.invalid"], "jobTitle": "Geschäftsführer"}})).await;
        c.check("Änderung übernommen", u.is_some_and(|u| u.to_string().contains("Geschäftsführer")), "fehlt");
        let _: Option<Value> = c.call("Kontakt löschen", "delete_contact", json!({"id": id})).await;
    }
    c.check("Ungültige E-Mail im Kontakt abgelehnt", c.raw("create_contact", json!({"accountId": acc, "input": {"emails": ["kaputt"]}})).await.is_err(), "angelegt");
    let orig: Option<AutoReply> = c.call("Abwesenheitsnotiz sichern", "get_auto_reply", json!({"accountId": acc})).await;
    if let Some(orig) = orig {
        let ar = json!({"status": "scheduled", "internalReplyMessage": "<p>Test – bin weg</p>", "externalReplyMessage": "<p>Test</p>",
            "scheduledStart": "2030-12-20T08:00:00Z", "scheduledEnd": "2031-01-06T17:00:00Z", "externalAudience": "contactsOnly"});
        let _: Option<Value> = c.call("Abwesenheitsnotiz planen (2030)", "set_auto_reply", json!({"accountId": acc, "autoReply": ar})).await;
        let back: Option<AutoReply> = c.call("Abwesenheitsnotiz lesen", "get_auto_reply", json!({"accountId": acc})).await;
        c.check("geplant gespeichert", back.is_some_and(|b| b.status == "scheduled"), "Status abweichend");
        let _: Option<Value> = c.call("Abwesenheitsnotiz wiederherstellen", "set_auto_reply", json!({"accountId": acc, "autoReply": orig})).await;
    }
    for (label, cmd, args) in [
        ("Nachricht mit unbekannter Kennung", "get_message", json!({"mailboxId": mb, "messageId": "AAkALgAAAAAAHYQDEapmEc2byACqAC-EWg0Abogus"})),
        ("Verschieben in unbekannten Ordner", "move_messages", json!({"mailboxId": mb, "ids": ["bogus"], "destFolderId": "bogus"})),
        ("Termin mit unbekannter Kennung", "get_event", json!({"accountId": acc, "eventId": "bogus"})),
    ] {
        let r = c.raw(cmd, args).await;
        c.check(&format!("{label} → Fehler"), r.is_err(), "kein Fehler");
    }
    let r = c.raw("list_folders", json!({"mailboxId": "gibt-es-nicht"})).await;
    c.check("Unbekanntes Postfach → Fehler oder leer", r.as_ref().map_or(true, |v| v.as_array().is_some_and(|a| a.is_empty())), format!("{r:?}"));

    // ---- cleanup ----
    println!("\n== Aufräumen");
    let mut n = 0;
    for wk in ["inbox", "sentitems", "archive", "snoozed", "drafts"] {
        if let Some(f) = c.folder(mb, wk).await {
            let ids: Vec<String> = c.find(mb, &f.id, &stamp).await.into_iter().map(|m| m.id).collect();
            if !ids.is_empty() {
                n += ids.len();
                let _ = c.raw("delete_messages", json!({"mailboxId": mb, "ids": ids})).await;
            }
        }
    }
    if let Some(f) = c.folder(mb, "deleteditems").await {
        let ids: Vec<String> = c.find(mb, &f.id, &stamp).await.into_iter().map(|m| m.id).collect();
        if !ids.is_empty() {
            let _ = c.raw("delete_messages", json!({"mailboxId": mb, "ids": ids})).await;
        }
    }
    c.note(format!("{n} Testmails gelöscht"));
    if let Some(p) = &parent {
        let _: Option<Value> = c.call("Testordner löschen", "delete_folder", json!({"mailboxId": mb, "folderId": p.id})).await;
    }
}
