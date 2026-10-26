//! Mail actions the server has not confirmed yet.
//!
//! The UI shows an action (read, archive, move, …) at once. The server request
//! follows, queued per mailbox so that actions reach the server in the order
//! they were made. Until the server confirms – and for a short grace period
//! after, while its other views (delta queries, another IMAP session) catch
//! up – syncs must not bring the old state back into the cache:
//! - a pending read state wins over what a sync reports,
//! - a mail on its way out of a folder stays hidden and is not re-added.
//!
//! A failed action is undone and the UI is told to reload.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::future::BoxFuture;

use super::Service;
use crate::error::Result;
use crate::model::{MessagePage, MessageSummary};

/// How long a confirmed change still wins over older server views.
const GRACE: Duration = Duration::from_secs(20);

#[derive(Default)]
struct Entry {
    read: Option<bool>,
    /// On its way out of this folder ("" = whichever it is in).
    gone: Option<String>,
    /// Actions of this mail still running.
    holds: u32,
    until: Option<Instant>,
}

impl Entry {
    fn live(&self, now: Instant) -> bool {
        self.holds > 0 || self.until.is_some_and(|u| u > now)
    }

    fn leaving(&self, folder_id: &str) -> bool {
        self.gone.as_deref().is_some_and(|f| f.is_empty() || f == folder_id)
    }
}

#[derive(Clone, Copy)]
enum Change {
    Read(bool),
    Gone,
}

#[derive(Clone, Default)]
pub(crate) struct Pending(Arc<Mutex<HashMap<(String, String), Entry>>>);

impl Pending {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<(String, String), Entry>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Read state set locally, until the server has it.
    pub(crate) fn hold_read(&self, mailbox_id: &str, ids: &[String], is_read: bool) -> Hold {
        let items: Vec<(String, String)> = ids.iter().map(|id| (id.clone(), String::new())).collect();
        self.hold(mailbox_id, &items, Change::Read(is_read))
    }

    /// Mails (id, folder) leaving their folder, until the server has moved them.
    pub(crate) fn hold_leaving(&self, mailbox_id: &str, items: &[(String, String)]) -> Hold {
        self.hold(mailbox_id, items, Change::Gone)
    }

    fn hold(&self, mailbox_id: &str, items: &[(String, String)], change: Change) -> Hold {
        let now = Instant::now();
        let mut map = self.lock();
        map.retain(|_, e| e.live(now));
        for (id, folder) in items {
            let e = map.entry((mailbox_id.to_string(), id.clone())).or_default();
            e.holds += 1;
            match change {
                Change::Read(r) => e.read = Some(r),
                Change::Gone => e.gone = Some(folder.clone()),
            }
        }
        let ids = items.iter().map(|(id, _)| id.clone()).collect();
        Hold { pending: self.clone(), mailbox_id: mailbox_id.to_string(), ids, change, confirmed: false }
    }

    fn with_live<R>(&self, mailbox_id: &str, id: &str, f: impl FnOnce(&Entry) -> R) -> Option<R> {
        let map = self.lock();
        map.get(&(mailbox_id.to_string(), id.to_string()))
            .filter(|e| e.live(Instant::now()))
            .map(f)
    }

    /// Whether the mail is on its way out (of whichever folder).
    pub(crate) fn is_gone(&self, mailbox_id: &str, id: &str) -> bool {
        self.with_live(mailbox_id, id, |e| e.gone.is_some()).unwrap_or(false)
    }

    pub(crate) fn is_leaving(&self, m: &MessageSummary) -> bool {
        self.with_live(&m.mailbox_id, &m.id, |e| e.leaving(&m.folder_id)).unwrap_or(false)
    }

    pub(crate) fn read(&self, mailbox_id: &str, id: &str) -> Option<bool> {
        self.with_live(mailbox_id, id, |e| e.read).flatten()
    }

    /// Server data about to enter the cache: false = leave it out (the mail is
    /// on its way out); a pending read state replaces the reported one.
    pub(crate) fn adjust(&self, m: &mut MessageSummary) -> bool {
        match self.with_live(&m.mailbox_id, &m.id, |e| (e.leaving(&m.folder_id), e.read)) {
            Some((true, _)) => false,
            Some((false, read)) => {
                if let Some(r) = read {
                    m.is_read = r;
                }
                true
            }
            None => true,
        }
    }

    pub(crate) fn adjust_all(&self, items: &mut Vec<MessageSummary>) {
        items.retain_mut(|m| self.adjust(m));
    }

    /// Ids of `mailbox_id` currently on their way out.
    pub(crate) fn gone_ids(&self, mailbox_id: &str) -> Vec<String> {
        let now = Instant::now();
        self.lock()
            .iter()
            .filter(|((mb, _), e)| mb == mailbox_id && e.gone.is_some() && e.live(now))
            .map(|((_, id), _)| id.clone())
            .collect()
    }
}

/// One running action; dropping it unconfirmed (error, early return) undoes
/// its pending state at once.
pub(crate) struct Hold {
    pending: Pending,
    mailbox_id: String,
    ids: Vec<String>,
    change: Change,
    confirmed: bool,
}

impl Hold {
    pub(crate) fn confirm(mut self) {
        self.confirmed = true;
    }
}

impl Drop for Hold {
    fn drop(&mut self) {
        let mut map = self.pending.lock();
        let until = Instant::now() + GRACE;
        for id in &self.ids {
            let key = (self.mailbox_id.clone(), id.clone());
            let Some(e) = map.get_mut(&key) else { continue };
            e.holds = e.holds.saturating_sub(1);
            if self.confirmed {
                e.until = Some(e.until.map_or(until, |u| u.max(until)));
            } else if e.holds == 0 {
                match self.change {
                    Change::Read(_) => e.read = None,
                    Change::Gone => e.gone = None,
                }
                if e.read.is_none() && e.gone.is_none() {
                    map.remove(&key);
                }
            }
        }
    }
}

impl Service {
    /// Serializes the mail actions of one mailbox (the queue towards the server).
    fn action_lock(&self, mailbox_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.action_locks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(mailbox_id.to_string())
            .or_default()
            .clone()
    }

    /// Runs `action` after the earlier actions of this mailbox (boxed: the
    /// actions are deep futures, the dispatcher's own would overflow the stack).
    pub(crate) async fn queued<T>(&self, mailbox_id: &str, action: BoxFuture<'_, Result<T>>) -> Result<T> {
        let lock = self.action_lock(mailbox_id);
        let _turn = lock.lock().await;
        action.await
    }

    /// Read/unread: the cache changes at once, the server follows in the queue.
    pub(crate) async fn set_read_queued(&self, mailbox_id: &str, ids: &[String], is_read: bool) -> Result<()> {
        let mut before = Vec::new();
        for id in ids {
            if let Some(m) = self.store.get_summary(mailbox_id, id)? {
                before.push((m.id, m.is_read));
            }
        }
        let hold = self.pending.hold_read(mailbox_id, ids, is_read);
        let folders = self.update_cached(mailbox_id, ids, |m| m.is_read = is_read)?;
        self.emit_folders(mailbox_id, folders);
        match self.queued(mailbox_id, Box::pin(self.set_read(mailbox_id, ids, is_read))).await {
            Ok(()) => {
                hold.confirm();
                Ok(())
            }
            Err(e) => {
                drop(hold);
                let mut folders = HashSet::new();
                for (id, was) in before {
                    folders.extend(self.update_cached(mailbox_id, &[id], |m| m.is_read = was)?);
                }
                self.emit_folders(mailbox_id, folders);
                Err(e)
            }
        }
    }

    /// An action that takes mails out of their folder (move, archive, snooze,
    /// delete): they disappear from the lists at once, the server follows in
    /// the queue. Mails already in `stays_in` do not leave their folder.
    pub(crate) async fn remove_queued<T>(
        &self,
        mailbox_id: &str,
        ids: &[String],
        stays_in: Option<&str>,
        action: BoxFuture<'_, Result<T>>,
    ) -> Result<T> {
        let mut leaving = Vec::new();
        for id in ids {
            let folder = self.store.get_summary(mailbox_id, id)?.map(|m| m.folder_id).unwrap_or_default();
            if stays_in.is_none_or(|dest| folder != dest) {
                leaving.push((id.clone(), folder));
            }
        }
        let hold = self.pending.hold_leaving(mailbox_id, &leaving);
        if !leaving.is_empty() {
            self.emit_mail_changed(mailbox_id, None);
        }
        let r = self.queued(mailbox_id, action).await;
        if r.is_ok() {
            hold.confirm();
        } else {
            drop(hold);
            self.emit_mail_changed(mailbox_id, None);
        }
        r
    }

    /// The cached page without the mails on their way out.
    pub(crate) fn hide_leaving(&self, page: &mut MessagePage) {
        let before = page.items.len();
        let unread_before = page.items.iter().filter(|m| !m.is_read).count();
        page.items.retain(|m| !self.pending.is_leaving(m));
        let removed = (before - page.items.len()) as i64;
        let removed_unread = (unread_before - page.items.iter().filter(|m| !m.is_read).count()) as i64;
        page.total = (page.total - removed).max(0);
        page.unread = (page.unread - removed_unread).max(0);
    }

    /// Folders of a mailbox with the counts as they will be once the pending
    /// removals are through.
    pub(crate) fn list_folders_for_ui(&self, mailbox_id: &str) -> Result<Vec<crate::model::Folder>> {
        let mut folders = self.store.list_folders(mailbox_id)?;
        for id in self.pending.gone_ids(mailbox_id) {
            if let Some(m) = self.store.get_summary(mailbox_id, &id)?.filter(|m| self.pending.is_leaving(m)) {
                if let Some(f) = folders.iter_mut().find(|f| f.id == m.folder_id) {
                    f.total_count = (f.total_count - 1).max(0);
                    if !m.is_read {
                        f.unread_count = (f.unread_count - 1).max(0);
                    }
                }
            }
        }
        Ok(folders)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(id: &str, read: bool) -> MessageSummary {
        MessageSummary {
            id: id.into(),
            mailbox_id: "mb".into(),
            folder_id: "inbox".into(),
            conversation_id: String::new(),
            subject: String::new(),
            from: None,
            to: vec![],
            received_at: String::new(),
            is_read: read,
            is_draft: false,
            importance: "normal".into(),
            flag: "notFlagged".into(),
            has_attachments: false,
            categories: vec![],
            preview: String::new(),
            due_at: None,
            recurrence: None,
            last_verb: None,
        }
    }

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn pending_read_wins_over_server_data() {
        let p = Pending::default();
        let h = p.hold_read("mb", &ids(&["a"]), true);
        let mut items = vec![msg("a", false), msg("b", false)];
        p.adjust_all(&mut items);
        assert!(items[0].is_read);
        assert!(!items[1].is_read);
        h.confirm();
        // Grace period: a late, stale server view still loses.
        let mut items = vec![msg("a", false)];
        p.adjust_all(&mut items);
        assert!(items[0].is_read);
    }

    #[test]
    fn leaving_mails_are_not_re_added() {
        let p = Pending::default();
        let _h = p.hold_leaving("mb", &[("a".into(), "inbox".into())]);
        let mut items = vec![msg("a", false), msg("b", false)];
        p.adjust_all(&mut items);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "b");
        assert!(p.is_gone("mb", "a"));
        assert!(!p.is_gone("other", "a"));
        // Where it arrives (a server that keeps ids on a move), it is shown.
        let mut arrived = msg("a", false);
        arrived.folder_id = "archive".into();
        assert!(!p.is_leaving(&arrived));
    }

    #[test]
    fn failed_action_releases_at_once() {
        let p = Pending::default();
        let h = p.hold_leaving("mb", &[("a".into(), "inbox".into())]);
        drop(h);
        assert!(!p.is_gone("mb", "a"));
        let h = p.hold_read("mb", &ids(&["a"]), false);
        drop(h);
        assert_eq!(p.read("mb", "a"), None);
    }

    #[test]
    fn newest_read_state_wins_while_both_run() {
        let p = Pending::default();
        let first = p.hold_read("mb", &ids(&["a"]), true);
        let second = p.hold_read("mb", &ids(&["a"]), false);
        first.confirm();
        assert_eq!(p.read("mb", "a"), Some(false));
        second.confirm();
        assert_eq!(p.read("mb", "a"), Some(false));
    }
}
