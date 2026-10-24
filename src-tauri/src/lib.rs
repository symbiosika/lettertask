//! The app itself; `main.rs` (desktop) and the Android/iOS entry point both
//! start it through `run()`.

#[cfg(target_os = "android")]
mod android;

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

    /// Mobile has a single window: the page shows compose/message/event/task
    /// as a full-screen layer (ui/js/mobile.js listens for this event).
    #[cfg(mobile)]
    fn open_window(&self, kind: &str, query: &str) -> mailcore::Result<()> {
        let _ = self.app.emit("open-window", serde_json::json!({ "kind": kind, "query": query }));
        Ok(())
    }
}

/// Largest file taken from a drop (the compose window checks the same).
const DROP_MAX: u64 = 150 * 1024 * 1024;

/// Files dropped onto a compose window. Tauri takes file drops over natively,
/// so the page never sees them (and the editor frame delivers no events in
/// WebKitGTK at all): the files are read here and handed to that window as
/// "files-dropped" `{ label, files: [{ name, contentBase64, size }], skipped: [name] }`.
fn on_window_event(window: &tauri::Window, event: &tauri::WindowEvent) {
    use tauri::{DragDropEvent, WindowEvent};
    let WindowEvent::DragDrop(dd) = event else { return };
    let label = window.label().to_string();
    if !label.starts_with("compose-") {
        return;
    }
    let app = window.app_handle().clone();
    match dd {
        DragDropEvent::Enter { .. } | DragDropEvent::Over { .. } => {
            let _ = app.emit_to(&label, "files-drag", serde_json::json!({ "label": label, "over": true }));
        }
        DragDropEvent::Leave => {
            let _ = app.emit_to(&label, "files-drag", serde_json::json!({ "label": label, "over": false }));
        }
        DragDropEvent::Drop { paths, .. } => {
            let paths = paths.clone();
            tauri::async_runtime::spawn_blocking(move || {
                use base64::Engine;
                let _ = app.emit_to(&label, "files-drag", serde_json::json!({ "label": label, "over": false }));
                let mut files = Vec::new();
                let mut skipped = Vec::new();
                for p in paths {
                    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    let size = std::fs::metadata(&p).ok().filter(|m| m.is_file()).map(|m| m.len());
                    match size.filter(|s| *s <= DROP_MAX).and_then(|_| std::fs::read(&p).ok()) {
                        Some(bytes) => files.push(serde_json::json!({
                            "name": name,
                            "size": bytes.len(),
                            "contentBase64": base64::engine::general_purpose::STANDARD.encode(&bytes),
                        })),
                        None => skipped.push(name),
                    }
                }
                let _ = app.emit_to(&label, "files-dropped", serde_json::json!({ "label": label, "files": files, "skipped": skipped }));
            });
        }
        _ => {}
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

/// Keeps every webview on the app's own pages. A web link that would navigate
/// a window or the mail iframe (WebKitGTK dispatches no click events from the
/// sandboxed frame, so the JS handler never sees it) opens in the browser.
fn navigation_guard() -> tauri::plugin::TauriPlugin<tauri::Wry> {
    tauri::plugin::Builder::new("navigation-guard")
        .on_navigation(|webview, url| {
            if !matches!(url.scheme(), "http" | "https") {
                return true;
            }
            let host = url.host_str().unwrap_or_default();
            if host.ends_with("localhost") || host == "127.0.0.1" {
                return true;
            }
            if let Err(e) = webview.app_handle().opener().open_url(url.as_str(), None::<&str>) {
                tracing::warn!("Link konnte nicht geöffnet werden: {e}");
            }
            false
        })
        .build()
}

/// The phone build runs against the built-in demo server unless it was built
/// with `LETTERTASK_DEMO=0` (real accounts).
pub(crate) fn mobile_demo() -> bool {
    cfg!(mobile) && option_env!("LETTERTASK_DEMO") != Some("0")
}

/// Options for the real (non-demo) service in `dir`.
pub(crate) fn real_options(dir: PathBuf) -> ServiceOptions {
    #[cfg(desktop)]
    let tokens: Arc<dyn TokenStore> = Arc::new(mailcore::tokens::KeyringTokenStore::new("lettertask", dir.join("tokens.json")));
    // No keyring crate backend on mobile; the app data dir is private to the
    // app. TODO: Android Keystore.
    #[cfg(mobile)]
    let tokens: Arc<dyn TokenStore> = Arc::new(mailcore::tokens::FileTokenStore::new(dir.join("tokens.json")));
    ServiceOptions {
        data_dir: dir,
        graph_base: mailcore::graph::DEFAULT_GRAPH_BASE.to_string(),
        authority: mailcore::auth::DEFAULT_AUTHORITY.to_string(),
        demo: false,
        token_store: tokens,
    }
}

/// Demo: a fake Graph server in this process and a throw-away data dir.
async fn demo_options(tmp: PathBuf) -> ServiceOptions {
    let fake = graph_fake::spawn(graph_fake::FakeOptions { seed_demo: true, now: chrono::Utc::now() }).await;
    let base = fake.base_url.clone();
    // Keep the fake server alive for the whole process.
    std::mem::forget(fake);
    ServiceOptions {
        data_dir: tmp.join(format!("lettertask-demo-{}", std::process::id())),
        graph_base: format!("{base}/v1.0"),
        authority: base,
        demo: true,
        token_store: Arc::new(MemoryTokenStore::default()),
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .init();
    let demo = mobile_demo() || std::env::args().any(|a| a == "--demo");

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(navigation_guard())
        .invoke_handler(tauri::generate_handler![call])
        .on_window_event(on_window_event)
        .setup(move |app| {
            let handle = app.handle().clone();
            let host: Arc<dyn mailcore::Host> = Arc::new(TauriHost { app: handle.clone() });
            // Android: the background worker or push service may have started
            // the service before the window; the window then joins it.
            #[cfg(target_os = "android")]
            let (host, existing) = (android::host_with_ui(host), android::service());
            #[cfg(not(target_os = "android"))]
            let existing: Option<Arc<Service>> = None;
            let svc = match existing {
                Some(svc) => svc,
                None => tauri::async_runtime::block_on(async move {
                    let opts = if demo {
                        // Android apps may not write to the system temp dir.
                        let tmp = if cfg!(mobile) {
                            handle.path().app_cache_dir().expect("app cache dir")
                        } else {
                            std::env::temp_dir()
                        };
                        demo_options(tmp).await
                    } else {
                        let app_dir = || handle.path().app_data_dir().expect("app data dir");
                        // Android: where the background worker looks, too.
                        #[cfg(target_os = "android")]
                        let dir = android::data_dir().unwrap_or_else(app_dir);
                        #[cfg(not(target_os = "android"))]
                        let dir = app_dir();
                        real_options(dir)
                    };
                    let svc = Service::new(opts, host)?;
                    if demo {
                        svc.ensure_demo_account().await?;
                    }
                    Ok::<_, mailcore::Error>(svc)
                })
                .map_err(|e| Box::new(std::io::Error::other(e.to_string())))?,
            };
            #[cfg(target_os = "android")]
            android::set_service(svc.clone());
            svc.start_background_sync();
            app.manage(svc);
            if let Some(w) = app.get_webview_window("main") {
                disable_pinch_zoom(&w);
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("Fehler beim Starten der Anwendung")
        .run(|app, event| {
            // Mails still in the send queue go out before the app quits.
            if let tauri::RunEvent::ExitRequested { api, .. } = event {
                let svc = app.state::<Arc<Service>>().inner().clone();
                if svc.pending_sends() > 0 {
                    api.prevent_exit();
                    let app = app.clone();
                    tauri::async_runtime::spawn(async move {
                        while svc.pending_sends() > 0 {
                            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                        }
                        app.exit(0);
                    });
                }
            }
        });
}

/// WebKitGTK zooms the whole page on a touchpad pinch, which is never wanted
/// in the app: a zoom gesture on the webview is claimed before WebKit sees it.
#[cfg(target_os = "linux")]
fn disable_pinch_zoom(w: &tauri::WebviewWindow) {
    let _ = w.with_webview(|wv| {
        use gtk::prelude::*;
        let view = wv.inner();
        let gesture = gtk::GestureZoom::new(&view);
        gesture.set_propagation_phase(gtk::PropagationPhase::Capture);
        gesture.connect_begin(|g, _| {
            g.set_state(gtk::EventSequenceState::Claimed);
        });
        // GTK3 gestures are not owned by their widget; keep it for the app's lifetime.
        std::mem::forget(gesture);
    });
}

#[cfg(not(target_os = "linux"))]
fn disable_pinch_zoom(_: &tauri::WebviewWindow) {}
