//! The app itself; `main.rs` (desktop) and the Android/iOS entry point both
//! start it through `run()`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use mailcore::tokens::{MemoryTokenStore, TokenStore};
use mailcore::{Host, Service, ServiceOptions};
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_opener::OpenerExt;

struct TauriHost {
    app: AppHandle,
}

#[async_trait]
impl Host for TauriHost {
    fn emit(&self, event: &str, payload: Value) {
        let _ = self.app.emit(event, payload);
    }

    fn open_url(&self, url: &str) -> mailcore::Result<()> {
        self.app
            .opener()
            .open_url(url, None::<&str>)
            .map_err(|e| mailcore::Error::Other(e.to_string()))
    }

    fn open_path(&self, path: &Path) -> mailcore::Result<()> {
        self.app
            .opener()
            .open_path(path.to_string_lossy(), None::<&str>)
            .map_err(|e| mailcore::Error::Other(e.to_string()))
    }

    async fn save_dialog(&self, suggested_name: &str) -> Option<PathBuf> {
        let app = self.app.clone();
        let name = suggested_name.to_string();
        let mut builder = app.dialog().file().set_file_name(&name);
        if let Ok(dir) = app.path().download_dir() {
            builder = builder.set_directory(dir);
        }
        tauri::async_runtime::spawn_blocking(move || builder.blocking_save_file())
            .await
            .ok()
            .flatten()
            .and_then(|p| p.into_path().ok())
    }

    fn notify(&self, title: &str, body: &str) {
        let _ = self
            .app
            .notification()
            .builder()
            .title(title)
            .body(body)
            .show();
    }

    #[cfg(desktop)]
    fn open_window(&self, kind: &str, query: &str) -> mailcore::Result<()> {
        use tauri::{WebviewUrl, WebviewWindowBuilder};
        let (title, w, h) = match kind {
            "compose" => ("Nachricht", 960.0, 760.0),
            "event" => ("Termin", 900.0, 760.0),
            _ => ("Nachricht", 1000.0, 800.0),
        };
        let label = format!("{kind}-{}", uuid::Uuid::new_v4().simple());
        let url = if query.is_empty() {
            format!("{kind}.html")
        } else {
            format!("{kind}.html?{query}")
        };
        WebviewWindowBuilder::new(&self.app, label, WebviewUrl::App(url.into()))
            .title(title)
            .inner_size(w, h)
            .min_inner_size(600.0, 450.0)
            .build()
            .map_err(|e| mailcore::Error::Other(e.to_string()))?;
        Ok(())
    }

    /// Mobile has a single window; separate compose/message/event windows
    /// still need an in-app navigation (proof of concept).
    #[cfg(mobile)]
    fn open_window(&self, _kind: &str, _query: &str) -> mailcore::Result<()> {
        Err(mailcore::Error::invalid("Auf dem Smartphone noch nicht verfügbar."))
    }
}

/// The only command: forwards to the service layer (see docs/API.md).
#[tauri::command]
async fn call(state: tauri::State<'_, Arc<Service>>, cmd: String, args: Option<Value>) -> Result<Value, String> {
    let svc = state.inner().clone();
    svc.dispatch(&cmd, args.unwrap_or(Value::Object(Default::default())))
        .await
        .map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .init();
    // Proof of concept: the mobile build always starts in demo mode.
    let demo = cfg!(mobile) || std::env::args().any(|a| a == "--demo");

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .invoke_handler(tauri::generate_handler![call])
        .setup(move |app| {
            let handle = app.handle().clone();
            let host = Arc::new(TauriHost { app: handle.clone() });
            let svc = tauri::async_runtime::block_on(async move {
                let (data_dir, graph_base, authority, tokens): (PathBuf, String, String, Arc<dyn TokenStore>) =
                    if demo {
                        let fake = graph_fake::spawn(graph_fake::FakeOptions {
                            seed_demo: true,
                            now: chrono::Utc::now(),
                        })
                        .await;
                        // Android apps may not write to the system temp dir.
                        let tmp = if cfg!(mobile) {
                            handle.path().app_cache_dir().expect("app cache dir")
                        } else {
                            std::env::temp_dir()
                        };
                        let dir = tmp.join(format!("lettertask-demo-{}", std::process::id()));
                        let base = fake.base_url.clone();
                        // Keep the fake server alive for the whole process.
                        std::mem::forget(fake);
                        (dir, format!("{base}/v1.0"), base, Arc::new(MemoryTokenStore::default()))
                    } else {
                        let dir = handle.path().app_data_dir().expect("app data dir");
                        #[cfg(desktop)]
                        let tokens = Arc::new(mailcore::tokens::KeyringTokenStore::new(
                            "lettertask",
                            dir.join("tokens.json"),
                        ));
                        // No keyring crate backend on mobile; the app data dir is
                        // private to the app. TODO: Android Keystore.
                        #[cfg(mobile)]
                        let tokens = Arc::new(mailcore::tokens::FileTokenStore::new(dir.join("tokens.json")));
                        (
                            dir,
                            mailcore::graph::DEFAULT_GRAPH_BASE.to_string(),
                            mailcore::auth::DEFAULT_AUTHORITY.to_string(),
                            tokens,
                        )
                    };
                let svc = Service::new(
                    ServiceOptions {
                        data_dir,
                        graph_base,
                        authority,
                        demo,
                        token_store: tokens,
                    },
                    host,
                )?;
                if demo {
                    svc.ensure_demo_account().await?;
                }
                svc.start_background_sync();
                Ok::<_, mailcore::Error>(svc)
            })
            .map_err(|e| Box::new(std::io::Error::other(e.to_string())))?;
            app.manage(svc);
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("Fehler beim Starten der Anwendung");
}
