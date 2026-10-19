//! Persistence of refresh tokens. Uses the OS credential store (Windows
//! Credential Manager, macOS Keychain, Secret Service on Linux) when available
//! and falls back to a file readable only by the current user.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use crate::error::Result;

pub trait TokenStore: Send + Sync {
    fn load(&self, account_id: &str) -> Option<String>;
    fn save(&self, account_id: &str, refresh_token: &str) -> Result<()>;
    fn delete(&self, account_id: &str);
}

#[derive(Default)]
pub struct MemoryTokenStore(Mutex<HashMap<String, String>>);

impl TokenStore for MemoryTokenStore {
    fn load(&self, account_id: &str) -> Option<String> {
        self.0.lock().unwrap().get(account_id).cloned()
    }
    fn save(&self, account_id: &str, refresh_token: &str) -> Result<()> {
        self.0
            .lock()
            .unwrap()
            .insert(account_id.to_string(), refresh_token.to_string());
        Ok(())
    }
    fn delete(&self, account_id: &str) {
        self.0.lock().unwrap().remove(account_id);
    }
}

/// JSON file `{account_id: refresh_token}` with 0600 permissions.
pub struct FileTokenStore {
    path: PathBuf,
    lock: Mutex<()>,
}

impl FileTokenStore {
    pub fn new(path: PathBuf) -> Self {
        FileTokenStore {
            path,
            lock: Mutex::new(()),
        }
    }

    fn read_all(&self) -> HashMap<String, String> {
        std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn write_all(&self, map: &HashMap<String, String>) -> Result<()> {
        let data = serde_json::to_string(map)?;
        let tmp = self.path.with_extension("tmp");
        std::fs::write(&tmp, data)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
        }
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

impl TokenStore for FileTokenStore {
    fn load(&self, account_id: &str) -> Option<String> {
        let _g = self.lock.lock().unwrap();
        self.read_all().get(account_id).cloned()
    }
    fn save(&self, account_id: &str, refresh_token: &str) -> Result<()> {
        let _g = self.lock.lock().unwrap();
        let mut all = self.read_all();
        all.insert(account_id.to_string(), refresh_token.to_string());
        self.write_all(&all)
    }
    fn delete(&self, account_id: &str) {
        let _g = self.lock.lock().unwrap();
        let mut all = self.read_all();
        if all.remove(account_id).is_some() {
            let _ = self.write_all(&all);
        }
    }
}

/// OS keyring with file fallback when no credential service is reachable.
#[cfg(feature = "keyring")]
pub struct KeyringTokenStore {
    service: String,
    fallback: FileTokenStore,
}

#[cfg(feature = "keyring")]
impl KeyringTokenStore {
    pub fn new(service: &str, fallback_path: PathBuf) -> Self {
        KeyringTokenStore {
            service: service.to_string(),
            fallback: FileTokenStore::new(fallback_path),
        }
    }

    fn entry(&self, account_id: &str) -> Option<keyring::Entry> {
        keyring::Entry::new(&self.service, account_id).ok()
    }
}

#[cfg(feature = "keyring")]
impl TokenStore for KeyringTokenStore {
    fn load(&self, account_id: &str) -> Option<String> {
        self.entry(account_id)
            .and_then(|e| e.get_password().ok())
            .or_else(|| self.fallback.load(account_id))
    }
    fn save(&self, account_id: &str, refresh_token: &str) -> Result<()> {
        if let Some(e) = self.entry(account_id) {
            if e.set_password(refresh_token).is_ok() {
                // Make sure no stale copy stays in the fallback file.
                self.fallback.delete(account_id);
                return Ok(());
            }
        }
        tracing::warn!("Kein Schlüsselbund verfügbar, speichere Token in Datei");
        self.fallback.save(account_id, refresh_token)
    }
    fn delete(&self, account_id: &str) {
        if let Some(e) = self.entry(account_id) {
            let _ = e.delete_credential();
        }
        self.fallback.delete(account_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_store_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let s = FileTokenStore::new(dir.path().join("tokens.json"));
        assert_eq!(s.load("a"), None);
        s.save("a", "r1").unwrap();
        s.save("b", "r2").unwrap();
        assert_eq!(s.load("a").as_deref(), Some("r1"));
        s.delete("a");
        assert_eq!(s.load("a"), None);
        assert_eq!(s.load("b").as_deref(), Some("r2"));
    }
}
