//! Unattended work, the same on every platform: one sync pass for a platform
//! scheduler (Android WorkManager) and the time the next Wiedervorlage is due,
//! so the platform can wake up for it. The desktop timer loop in `mod.rs` runs
//! the same pass itself.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::Serialize;

use super::Service;
use crate::model::ListQuery;

/// Outcome of `background_sync`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundRun {
    /// Nothing failed (a skipped pass counts as ok: there is nothing to retry).
    pub ok: bool,
    /// Why the pass did not sync: "focus" while a Fokuszeit is running.
    pub skipped: Option<String>,
    /// When the next Wiedervorlage is due (RFC 3339), for a platform wake-up.
    pub next_wakeup: Option<String>,
}

/// What a platform scheduler should keep running (Android: WorkManager
/// interval and whether the push service is needed).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundPlan {
    /// Minutes between background passes; 0 = none (no accounts, or "never").
    pub interval_mins: u32,
    /// Push is on and at least one account can use it (IMAP IDLE).
    pub push: bool,
}

impl Service {
    pub fn background_plan(&self) -> BackgroundPlan {
        let c = self.config();
        let accounts = self.store.list_accounts().unwrap_or_default();
        let interval_mins = if accounts.is_empty() { 0 } else { c.background_interval_mins };
        let push = c.push_enabled && accounts.iter().any(|a| crate::imap::is_imap_id(&a.id));
        BackgroundPlan { interval_mins, push }
    }

    /// One sync pass while nobody looks at the app: due Wiedervorlagen, folders
    /// and inbox of every mailbox, new-mail notifications. Fokuszeit pauses it,
    /// like the desktop loop.
    pub async fn background_sync(self: &Arc<Self>) -> BackgroundRun {
        let skipped = self.focus_until().is_some().then(|| "focus".to_string());
        let ok = skipped.is_some() || self.sync_accounts(true, |_| true).await;
        self.apply_push();
        BackgroundRun { ok, skipped, next_wakeup: self.next_snooze_due().map(|d| d.to_rfc3339()) }
    }

    /// Earliest due time of a mail on Wiedervorlage (from the local cache).
    pub fn next_snooze_due(&self) -> Option<DateTime<Utc>> {
        let accounts = self.store.list_accounts().ok()?;
        accounts
            .iter()
            .flat_map(|a| a.mailboxes.iter())
            .filter_map(|mb| {
                let folder = self.store.folder_by_well_known(&mb.id, "snoozed").ok()??;
                let q = ListQuery { mailbox_id: mb.id.clone(), folder_id: folder.id, limit: 10_000, ..Default::default() };
                self.store.list_messages(&q).ok()
            })
            .flat_map(|page| page.items)
            .filter_map(|m| m.due_at)
            .filter_map(|d| DateTime::parse_from_rfc3339(&d).ok())
            .map(|d| d.with_timezone(&Utc))
            .min()
    }

    /// Whether someone uses the app window. The phone clears it while the app
    /// is in the background: the timer loop then pauses and the platform
    /// scheduler takes over (one pass per interval instead of every minute).
    /// On the desktop it stays set.
    pub fn set_interactive(&self, on: bool) {
        self.interactive.store(on, Ordering::Relaxed);
    }

    pub(crate) fn is_interactive(&self) -> bool {
        self.interactive.load(Ordering::Relaxed)
    }
}
