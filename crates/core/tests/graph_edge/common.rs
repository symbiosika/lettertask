//! Shared setup for the Graph edge-case tests: a fresh fake Graph server and a
//! signed-in demo account, driven through `Service::dispatch` like the UI.

use std::sync::Arc;

use mailcore::host::NullHost;
use mailcore::model::*;
use mailcore::tokens::MemoryTokenStore;
use mailcore::{Service, ServiceOptions};
use serde_json::{json, Value};

pub const ME: &str = "max.mustermann@contoso.example";

pub struct Ctx {
    pub svc: Arc<Service>,
    pub host: Arc<NullHost>,
    pub fake: graph_fake::FakeServer,
    pub dir: tempfile::TempDir,
    pub account: String,
}

/// Fake with demo seed, reference time fixed to `now`.
pub async fn setup_at(now: chrono::DateTime<chrono::Utc>) -> Ctx {
    let fake = graph_fake::spawn(graph_fake::FakeOptions { seed_demo: true, now }).await;
    let dir = tempfile::tempdir().unwrap();
    let host = Arc::new(NullHost::default());
    let svc = Service::new(
        ServiceOptions {
            data_dir: dir.path().to_path_buf(),
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
    let account = accounts[0].id.clone();
    Ctx { svc, host, fake, dir, account }
}

pub async fn setup() -> Ctx {
    setup_at(chrono::Utc::now()).await
}

pub async fn call<T: serde::de::DeserializeOwned>(svc: &Arc<Service>, cmd: &str, args: Value) -> T {
    let v = svc
        .dispatch(cmd, args.clone())
        .await
        .unwrap_or_else(|e| panic!("{cmd}({args}) failed: {e}"));
    serde_json::from_value(v).unwrap_or_else(|e| panic!("{cmd}: cannot decode result: {e}"))
}

pub async fn call_err(svc: &Arc<Service>, cmd: &str, args: Value) -> String {
    match svc.dispatch(cmd, args).await {
        Ok(v) => panic!("{cmd} unexpectedly succeeded: {v}"),
        Err(e) => e.to_string(),
    }
}

impl Ctx {
    pub async fn folders(&self) -> Vec<Folder> {
        call(&self.svc, "list_folders", json!({"mailboxId": self.account})).await
    }

    pub async fn folder(&self, wk: &str) -> Folder {
        self.folders()
            .await
            .into_iter()
            .find(|f| f.well_known.as_deref() == Some(wk))
            .unwrap_or_else(|| panic!("folder {wk} missing"))
    }

    pub async fn list(&self, folder: &str) -> MessagePage {
        call(
            &self.svc,
            "list_messages",
            json!({"mailboxId": self.account, "folderId": folder, "sortBy": "received", "sortDesc": true, "offset": 0, "limit": 5000}),
        )
        .await
    }

    pub async fn sync(&self, folder: &str) -> SyncResult {
        call(&self.svc, "sync_folder", json!({"mailboxId": self.account, "folderId": folder})).await
    }

    pub async fn default_calendar(&self) -> String {
        let cals: Vec<Calendar> = call(&self.svc, "list_calendars", json!({"accountId": self.account})).await;
        cals.into_iter().find(|c| c.is_default).expect("default calendar").id
    }

    pub async fn create_event(&self, calendar: &str, input: Value) -> Event {
        call(&self.svc, "create_event", json!({"accountId": self.account, "calendarId": calendar, "input": input})).await
    }

    pub async fn events(&self, calendar: &str, start: &str, end: &str) -> Vec<Event> {
        call(
            &self.svc,
            "list_events",
            json!({"accountId": self.account, "calendarIds": [calendar], "start": start, "end": end}),
        )
        .await
    }

    pub fn emitted(&self, name: &str) -> Vec<Value> {
        self.host
            .events
            .lock()
            .unwrap()
            .iter()
            .filter(|(e, _)| e == name)
            .map(|(_, p)| p.clone())
            .collect()
    }
}

/// Minimal event input in Europe/Berlin.
pub fn ev_input(subject: &str, start: &str, end: &str) -> Value {
    json!({
        "subject": subject, "start": start, "end": end, "timeZone": "Europe/Berlin",
        "isAllDay": false, "location": "", "bodyHtml": "", "attendees": [], "showAs": "busy",
        "categories": [], "isReminderOn": false, "reminderMinutes": 0, "isOnlineMeeting": false,
        "recurrence": null, "sensitivity": "normal", "importance": "normal"
    })
}
