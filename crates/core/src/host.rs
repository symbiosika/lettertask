//! What the service needs from the surrounding application (Tauri window or
//! the HTTP dev server): events to the UI, opening URLs/files/windows, dialogs.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::Value;

use crate::error::Result;

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
