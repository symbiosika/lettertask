//! "Sofort benachrichtigen": while `push_enabled` is set, every IMAP account
//! (Gmail included) keeps a second connection in IDLE on its inbox. When the
//! server reports a change, the inbox syncs at once and new mail is announced
//! as usual. The same on desktop and phone; on the phone a foreground service
//! keeps the process alive and an alarm calls `refresh_push` after the device
//! slept, as timers do not run in Doze.
//!
//! Microsoft 365 (Graph) has no IDLE: those accounts stay on the periodic sync.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use async_imap::extensions::idle::IdleResponse;
use serde::Serialize;
use tokio::sync::Notify;

use super::Service;
use crate::error::{Error, Result};
use crate::imap::conn::timed;

/// IDLE is renewed before servers and NAT routers drop a silent connection
/// (RFC 2177: at least every 29 minutes; 24 like Thunderbird).
const RENEW: Duration = Duration::from_secs(24 * 60);
const DONE_TIMEOUT: Duration = Duration::from_secs(30);
const RETRY_MIN: Duration = Duration::from_secs(30);
const RETRY_MAX: Duration = Duration::from_secs(15 * 60);

/// What one account's listener is doing (shown in the settings).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PushInfo {
    pub account_id: String,
    pub email: String,
    /// "connecting" | "listening" | "unsupported" | "error"
    pub state: &'static str,
    pub message: Option<String>,
}

struct Listener {
    task: tokio::task::JoinHandle<()>,
    refresh: Arc<Notify>,
    info: Arc<Mutex<PushInfo>>,
}

#[derive(Default)]
pub(crate) struct PushState {
    listeners: Mutex<HashMap<String, Listener>>,
}

impl Drop for PushState {
    fn drop(&mut self) {
        for l in self.listeners.get_mut().unwrap().values() {
            l.task.abort();
        }
    }
}

enum Ended {
    /// The server has no IDLE: nothing to retry.
    Unsupported,
}

impl Service {
    /// Starts or stops listeners so they match the setting and the accounts.
    /// Cheap; called after config changes and from the sync passes.
    pub fn apply_push(self: &Arc<Self>) {
        let wanted: Vec<(String, String)> = if self.config().push_enabled {
            self.store
                .list_accounts()
                .unwrap_or_default()
                .into_iter()
                .filter(|a| crate::imap::is_imap_id(&a.id))
                .map(|a| (a.id, a.email))
                .collect()
        } else {
            Vec::new()
        };
        let mut listeners = self.push.listeners.lock().unwrap();
        listeners.retain(|id, l| {
            let keep = wanted.iter().any(|(w, _)| w == id);
            if !keep {
                l.task.abort();
            }
            keep
        });
        for (id, email) in wanted {
            if listeners.contains_key(&id) {
                continue;
            }
            let refresh = Arc::new(Notify::new());
            let info = Arc::new(Mutex::new(PushInfo { account_id: id.clone(), email, state: "connecting", message: None }));
            let task = tokio::spawn(listen(Arc::downgrade(self), id.clone(), refresh.clone(), info.clone()));
            listeners.insert(id, Listener { task, refresh, info });
        }
    }

    /// Renews every IDLE now and catches up on the inbox (phone: after Doze,
    /// or when the network changed). Listeners waiting to retry try at once.
    pub fn refresh_push(&self) {
        for l in self.push.listeners.lock().unwrap().values() {
            l.refresh.notify_one();
        }
    }

    pub fn push_status(&self) -> Vec<PushInfo> {
        let mut v: Vec<PushInfo> =
            self.push.listeners.lock().unwrap().values().map(|l| l.info.lock().unwrap().clone()).collect();
        v.sort_by(|a, b| a.email.cmp(&b.email));
        v
    }

    /// One IDLE connection on the inbox until it fails.
    async fn idle_inbox(&self, account_id: &str, refresh: &Notify, info: &Mutex<PushInfo>) -> Result<Ended> {
        let inbox = self
            .store
            .folder_by_well_known(account_id, "inbox")?
            .map(|f| f.id)
            .unwrap_or_else(|| "INBOX".into());
        // A connection of its own: the cached one stays free for the sync.
        let mut conn = self.imap_connect(account_id).await?;
        if !timed(conn.s().capabilities()).await?.has_str("IDLE") {
            return Ok(Ended::Unsupported);
        }
        timed(conn.s().select(&inbox)).await?;
        // Whatever arrived while nobody listened.
        self.push_sync(account_id, &inbox).await;
        set(info, "listening", None);
        let mut session = conn.session;
        loop {
            let mut idle = session.idle();
            timed(idle.init()).await?;
            let r = {
                let (wait, stop) = idle.wait_with_timeout(RENEW);
                tokio::pin!(wait);
                let mut stop = Some(stop);
                tokio::select! {
                    r = &mut wait => r,
                    _ = refresh.notified() => {
                        // Dropping the stop source ends the wait.
                        stop.take();
                        wait.await
                    }
                }
            };
            let r = r.map_err(|e| Error::Network(format!("IMAP IDLE: {e}")))?;
            session = tokio::time::timeout(DONE_TIMEOUT, idle.done())
                .await
                .map_err(|_| Error::Network("Zeitüberschreitung beim IMAP-Server".into()))?
                .map_err(|e| Error::Network(format!("IMAP IDLE: {e}")))?;
            match r {
                IdleResponse::NewData(_) | IdleResponse::ManualInterrupt => self.push_sync(account_id, &inbox).await,
                IdleResponse::Timeout => {}
            }
        }
    }

    /// Inbox sync from a push signal. Fokuszeit holds new mail back; the
    /// sync after the Fokuszeit picks it up.
    async fn push_sync(&self, account_id: &str, inbox: &str) {
        if self.focus_until().is_some() {
            return;
        }
        if let Err(e) = self.sync_folder(account_id, inbox, false).await {
            tracing::warn!("Push-Abgleich {account_id}: {e}");
        }
    }
}

fn set(info: &Mutex<PushInfo>, state: &'static str, message: Option<String>) {
    let mut i = info.lock().unwrap();
    i.state = state;
    i.message = message;
}

/// Keeps one account listening; reconnects with growing pauses after errors.
async fn listen(svc: Weak<Service>, account_id: String, refresh: Arc<Notify>, info: Arc<Mutex<PushInfo>>) {
    let mut pause = RETRY_MIN;
    loop {
        let Some(s) = svc.upgrade() else { return };
        set(&info, "connecting", None);
        match s.idle_inbox(&account_id, &refresh, &info).await {
            Ok(Ended::Unsupported) => {
                set(&info, "unsupported", Some("Der Server unterstützt kein IDLE – Abruf im eingestellten Intervall.".into()));
                return;
            }
            Err(e) => {
                // A connection that listened for a while starts the pauses over.
                if info.lock().unwrap().state == "listening" {
                    pause = RETRY_MIN;
                }
                tracing::info!("Push {account_id}: {e}");
                set(&info, "error", Some(e.to_string()));
            }
        }
        drop(s);
        tokio::select! {
            _ = tokio::time::sleep(pause) => {}
            _ = refresh.notified() => {}
        }
        pause = (pause * 2).min(RETRY_MAX);
    }
}
