//! What the service needs from the surrounding application (Tauri window or
//! the HTTP dev server): events to the UI, opening URLs/files/windows, dialogs.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::error::{Error, Result};

#[async_trait]
pub trait Host: Send + Sync {
    /// Broadcast an event to all UI windows.
    fn emit(&self, event: &str, payload: Value);
    /// Open a URL in the system browser.
    fn open_url(&self, url: &str) -> Result<()>;
    /// Open a local file with the default application.
    fn open_path(&self, path: &Path) -> Result<()>;
    /// Ask the user where to save a file. `None` = cancelled.
    async fn save_dialog(&self, suggested_name: &str) -> Option<PathBuf>;
    /// Desktop notification.
    fn notify(&self, title: &str, body: &str);
    /// Open `ui/<kind>.html?<query>` in a new window.
    fn open_window(&self, kind: &str, query: &str) -> Result<()>;
}

/// Wraps the application's host so that, whatever code path asks for it,
/// nothing but web links reaches the browser and no executable attachment is
/// handed to the OS "open" action.
pub(crate) struct Guarded(pub Arc<dyn Host>);

#[async_trait]
impl Host for Guarded {
    fn emit(&self, event: &str, payload: Value) {
        self.0.emit(event, payload)
    }
    fn open_url(&self, url: &str) -> Result<()> {
        let lower = url.trim_start().to_ascii_lowercase();
        if !(lower.starts_with("https://") || lower.starts_with("http://")) {
            return Err(Error::invalid("Nur http(s)- und mailto-Links können geöffnet werden."));
        }
        self.0.open_url(url)
    }
    fn open_path(&self, path: &Path) -> Result<()> {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if crate::files::is_risky_to_open(&name) {
            return Err(Error::invalid(format!(
                "„{name}“ kann ausführbaren Code enthalten und wird aus Sicherheitsgründen nicht direkt geöffnet. \
                 Speichern Sie den Anhang („Speichern unter…“) nur, wenn Sie dem Absender vertrauen."
            )));
        }
        self.0.open_path(path)
    }
    async fn save_dialog(&self, suggested_name: &str) -> Option<PathBuf> {
        self.0.save_dialog(suggested_name).await
    }
    fn notify(&self, title: &str, body: &str) {
        self.0.notify(title, body)
    }
    fn open_window(&self, kind: &str, query: &str) -> Result<()> {
        if !matches!(kind, "compose" | "message" | "event" | "task") {
            return Err(Error::invalid("Unbekannter Fenstertyp"));
        }
        self.0.open_window(kind, query)
    }
}

/// Host that does nothing; used in tests.
#[derive(Default)]
pub struct NullHost {
    pub events: std::sync::Mutex<Vec<(String, Value)>>,
}

#[async_trait]
impl Host for NullHost {
    fn emit(&self, event: &str, payload: Value) {
        self.events.lock().unwrap().push((event.to_string(), payload));
    }
    fn open_url(&self, _url: &str) -> Result<()> {
        Ok(())
    }
    fn open_path(&self, _path: &Path) -> Result<()> {
        Ok(())
    }
    async fn save_dialog(&self, suggested_name: &str) -> Option<PathBuf> {
        Some(std::env::temp_dir().join(suggested_name))
    }
    fn notify(&self, _title: &str, _body: &str) {}
    fn open_window(&self, _kind: &str, _query: &str) -> Result<()> {
        Ok(())
    }
}
