use std::collections::HashSet;
use std::sync::Arc;

use base64::Engine;
use chrono::{Duration as ChronoDuration, SecondsFormat, Utc};
use futures::{stream, StreamExt, TryStreamExt};
use serde_json::{json, Value};

use super::Service;
use crate::convert::{self, MESSAGE_DETAIL_SELECT, MESSAGE_SUMMARY_SELECT};
use crate::error::{Error, Result};
use crate::graph::{seg, GraphClient};
use crate::model::*;

pub(crate) const WELL_KNOWN: &[&str] = &[
    "inbox",
    "drafts",
    "sentitems",
    "deleteditems",
    "archive",
    "junkemail",
    "outbox",
];

/// How far back the initial delta sync of a folder reaches.
const INITIAL_SYNC_DAYS: i64 = 60;
const PAGE_SIZE: usize = 100;
const PARALLEL: usize = 6;

fn enc(s: &str) -> String {
    urlencoding::encode(s).into_owned()
}

impl Service {
    // ---- folders ----

    pub(crate) async fn sync_folders(&self, mailbox_id: &str) -> Result<Vec<Folder>> {
        if crate::imap::is_imap_id(mailbox_id) {
            return self.imap_sync_folders(mailbox_id).await;
        }
        let (mb, c) = self.mailbox(mailbox_id)?;
        let prefix = mb.graph_prefix();
        let known: Vec<(String, String)> = self
            .store
            .list_folders(mailbox_id)?
            .into_iter()
            .filter_map(|f| f.well_known.map(|w| (f.id, w)))
            .collect();
        let root_key = format!("rootfolder|{mailbox_id}");
        let cached_root = self.store.kv_get(&root_key)?;
        let (root_id, wk) = if known.iter().any(|(_, w)| w == "inbox") && cached_root.is_some() {
            (cached_root, known)
        } else {
            let root_id = c
                .get(&format!("{prefix}/mailFolders/msgfolderroot?$select=id"))
                .await
                .ok()
                .and_then(|v| v["id"].as_str().map(str::to_string));
            if let Some(r) = &root_id {
                self.store.kv_set(&root_key, r)?;
            }
            // Resolve well-known folder ids (some may not exist, e.g. archive).
            let wk: Vec<(String, String)> = stream::iter(WELL_KNOWN.iter().map(|n| n.to_string()).collect::<Vec<_>>())
                .map(|name: String| {
                    let c = c.clone();
                    let prefix = prefix.clone();
                    async move {
                        c.get(&format!("{prefix}/mailFolders/{name}?$select=id"))
                            .await
                            .ok()
                            .and_then(|v| v["id"].as_str().map(|id| (id.to_string(), name)))
                    }
                })
                .buffer_unordered(PARALLEL)
                .filter_map(|x| async move { x })
                .collect()
                .await;
            (root_id, wk)
        };

        // Breadth-first over the folder hierarchy.
        let mut out = Vec::new();
        let mut queue = vec![format!("{prefix}/mailFolders?$top=250&includeHiddenFolders=false")];
        while !queue.is_empty() {
            let pages: Vec<Vec<Value>> = stream::iter(queue.drain(..))
                .map(|p| {
                    let c = c.clone();
                    async move { c.get_all(&p, &[], 5000).await }
                })
                .buffer_unordered(PARALLEL)
                .try_collect()
                .await?;
            for v in pages.into_iter().flatten() {
                if v["isHidden"].as_bool().unwrap_or(false) {
                    continue;
                }
                let id = v["id"].as_str().unwrap_or("").to_string();
                let well_known = wk
                    .iter()
                    .find(|(wid, _)| *wid == id)
                    .map(|(_, n)| n.to_string());
                let mut f = convert::folder(&v, mailbox_id, root_id.as_deref(), well_known);
                if f.well_known.is_none() && self.is_snooze_folder(mailbox_id, &f)? {
                    f.well_known = Some("snoozed".into());
                }
                if f.child_folder_count > 0 {
                    queue.push(format!(
                        "{prefix}/mailFolders/{}/childFolders?$top=250",
                        seg(&f.id)
                    ));
                }
                out.push(f);
            }
        }
        self.store.replace_folders(mailbox_id, &out)?;
        self.store.list_folders(mailbox_id)
    }

    pub(crate) async fn create_folder(&self, mailbox_id: &str, parent_id: Option<&str>, name: &str) -> Result<Folder> {
        if crate::imap::is_imap_id(mailbox_id) {
            return self.imap_create_folder(mailbox_id, parent_id, name).await;
        }
        let name = name.trim();
        if name.is_empty() {
            return Err(Error::invalid("Ordnername darf nicht leer sein."));
        }
        let (mb, c) = self.mailbox(mailbox_id)?;
        let path = match parent_id {
            Some(p) => format!("{}/mailFolders/{}/childFolders", mb.graph_prefix(), seg(p)),
            None => format!("{}/mailFolders", mb.graph_prefix()),
        };
        let v = c.post(&path, &json!({"displayName": name})).await?;
        let mut f = convert::folder(&v, mailbox_id, None, None);
        f.parent_id = parent_id.map(str::to_string);
        self.store.upsert_folder(&f)?;
        if let Some(p) = parent_id {
            if let Some(mut parent) = self.store.get_folder(mailbox_id, p)? {
                parent.child_folder_count += 1;
                self.store.upsert_folder(&parent)?;
            }
        }
        self.emit_mail_changed(mailbox_id, None);
        Ok(f)
    }

    pub(crate) async fn rename_folder(&self, mailbox_id: &str, folder_id: &str, name: &str) -> Result<Folder> {
        if crate::imap::is_imap_id(mailbox_id) {
            return self.imap_rename_folder(mailbox_id, folder_id, name).await;
        }
        let (mb, c) = self.mailbox(mailbox_id)?;
        let name = name.trim();
        if name.is_empty() {
            return Err(Error::invalid("Ordnername darf nicht leer sein."));
        }
        c.patch(
            &format!("{}/mailFolders/{}", mb.graph_prefix(), seg(folder_id)),
            &json!({"displayName": name}),
        )
        .await?;
        let mut f = self
            .store
            .get_folder(mailbox_id, folder_id)?
            .ok_or_else(|| Error::NotFound("Ordner".into()))?;
        f.display_name = name.to_string();
        self.store.upsert_folder(&f)?;
        self.emit_mail_changed(mailbox_id, None);
        Ok(f)
    }

    pub(crate) async fn delete_folder(&self, mailbox_id: &str, folder_id: &str) -> Result<()> {
        if crate::imap::is_imap_id(mailbox_id) {
            return self.imap_delete_folder(mailbox_id, folder_id).await;
        }
        if let Some(f) = self.store.get_folder(mailbox_id, folder_id)? {
            if f.well_known.is_some() {
                return Err(Error::invalid("Standardordner können nicht gelöscht werden."));
            }
        }
        let (mb, c) = self.mailbox(mailbox_id)?;
        c.delete(&format!("{}/mailFolders/{}", mb.graph_prefix(), seg(folder_id)))
            .await?;
        self.store.delete_folder(mailbox_id, folder_id)?;
        // Deleted folders move to Deleted Items on the server; refresh the tree.
        let _ = self.sync_folders(mailbox_id).await;
        self.emit_mail_changed(mailbox_id, None);
        Ok(())
    }

    pub(crate) async fn mark_folder_read(&self, mailbox_id: &str, folder_id: &str) -> Result<()> {
        if crate::imap::is_imap_id(mailbox_id) {
            return self.imap_mark_folder_read(mailbox_id, folder_id).await;
        }
        let (mb, c) = self.mailbox(mailbox_id)?;
        let unread = c
            .get_all(
                &format!(
                    "{}/mailFolders/{}/messages?$select=id&$top=250&$filter={}",
                    mb.graph_prefix(),
                    seg(folder_id),
                    enc("isRead eq false")
                ),
                &[],
                10_000,
            )
            .await?;
        let ids: Vec<String> = unread
            .iter()
            .filter_map(|v| v["id"].as_str().map(str::to_string))
            .collect();
        self.patch_many(&mb, &c, &ids, json!({"isRead": true})).await?;
        for id in self.store.folder_message_ids(mailbox_id, folder_id, true)? {
            if let Some(mut m) = self.store.get_summary(mailbox_id, &id)? {
                m.is_read = true;
                self.store.upsert_messages(&[m])?;
            }
        }
        if let Some(mut f) = self.store.get_folder(mailbox_id, folder_id)? {
            f.unread_count = 0;
            self.store.upsert_folder(&f)?;
        }
        self.emit_mail_changed(mailbox_id, Some(folder_id));
        Ok(())
    }

    pub(crate) async fn empty_folder(&self, mailbox_id: &str, folder_id: &str) -> Result<()> {
        if crate::imap::is_imap_id(mailbox_id) {
            return self.imap_empty_folder(mailbox_id, folder_id).await;
        }
        let (mb, c) = self.mailbox(mailbox_id)?;
        let all = c
            .get_all(
                &format!(
                    "{}/mailFolders/{}/messages?$select=id&$top=250",
                    mb.graph_prefix(),
                    seg(folder_id)
                ),
                &[],
                100_000,
            )
            .await?;
        let prefix = mb.graph_prefix();
        let ids: Vec<String> = all
            .iter()
            .filter_map(|v| v["id"].as_str().map(str::to_string))
            .collect();
        stream::iter(ids)
            .map(|id| {
                let c = c.clone();
                let p = format!("{prefix}/messages/{}", seg(&id));
                async move { c.delete(&p).await }
            })
            .buffer_unordered(PARALLEL)
            .try_collect::<Vec<_>>()
            .await?;
        self.store.delete_folder_messages(mailbox_id, folder_id)?;
        if let Some(mut f) = self.store.get_folder(mailbox_id, folder_id)? {
            f.unread_count = 0;
            f.total_count = 0;
            self.store.upsert_folder(&f)?;
        }
        self.emit_mail_changed(mailbox_id, Some(folder_id));
        Ok(())
    }

    // ---- message list & sync ----

    pub(crate) fn list_messages(&self, q: &ListQuery) -> Result<MessagePage> {
        let mut page = self.store.list_messages(q)?;
        let unfiltered = !q.unread_only && !q.flagged_only && q.category.is_none() && q.text.as_deref().unwrap_or("").is_empty();
        if let Some(f) = self.store.get_folder(&q.mailbox_id, &q.folder_id)? {
            let cached = if unfiltered {
                page.total
            } else {
                self.store
                    .list_messages(&ListQuery {
                        mailbox_id: q.mailbox_id.clone(),
                        folder_id: q.folder_id.clone(),
                        limit: 1,
                        ..Default::default()
                    })?
                    .total
            };
            page.can_load_older = !self.store.is_fully_loaded(&q.mailbox_id, &q.folder_id)? && cached < f.total_count;
        }
        Ok(page)
    }

    /// Delta sync of one folder. `quiet` suppresses new-mail notifications.
    pub(crate) async fn sync_folder(&self, mailbox_id: &str, folder_id: &str, quiet: bool) -> Result<SyncResult> {
        if crate::imap::is_imap_id(mailbox_id) {
            return self.imap_sync_folder(mailbox_id, folder_id, quiet).await;
        }
        let lock = self.folder_lock(mailbox_id, folder_id);
        let _g = lock.lock().await;
        let (mb, c) = self.mailbox(mailbox_id)?;
        let initial = self.store.delta_link(mailbox_id, folder_id)?.is_none();
        let res = match self.run_delta(&mb, &c, folder_id).await {
            Err(Error::Graph { status, .. }) if status == 410 || status == 400 => {
                // Sync state expired: start over.
                self.store.set_delta_link(mailbox_id, folder_id, None)?;
                self.run_delta(&mb, &c, folder_id).await?
            }
            other => other?,
        };
        let (mut result, mut new_unread) = res;
        match self.repair_incomplete(&mb, &c, folder_id).await {
            Ok(n) => result.updated += n as i64,
            Err(e) => tracing::warn!("repair of incomplete messages failed: {e}"),
        }
        if !initial && !new_unread.is_empty() && self.is_inbox(mailbox_id, folder_id) {
            // Rule chain runs before the new mails are announced to the UI.
            let moved = self.apply_inbox_rules(mailbox_id, &new_unread).await;
            new_unread.retain(|m| !moved.contains(&m.id));
        }
        if initial {
            // Fill up folders whose recent window is sparse (e.g. Archive).
            let page = self.store.list_messages(&ListQuery {
                mailbox_id: mailbox_id.into(),
                folder_id: folder_id.into(),
                limit: 1,
                ..Default::default()
            })?;
            if page.total < 50 {
                drop(_g);
                let _ = self.load_older(mailbox_id, folder_id).await;
            }
        }
        if result.added + result.updated + result.removed > 0 {
            self.refresh_folder_counts(&mb, &c, folder_id).await;
            self.emit_mail_changed(mailbox_id, Some(folder_id));
        }
        let is_inbox = self
            .store
            .get_folder(mailbox_id, folder_id)?
            .and_then(|f| f.well_known)
            .as_deref()
            == Some("inbox");
        if !quiet && !initial && is_inbox && !new_unread.is_empty() {
            let first = &new_unread[0];
            let from = first
                .from
                .as_ref()
                .map(|a| if a.name.is_empty() { a.address.clone() } else { a.name.clone() })
                .unwrap_or_default();
            self.emit(
                "new-mail",
                json!({"mailboxId": mailbox_id, "count": new_unread.len(), "from": from, "subject": first.subject}),
            );
            let title = if new_unread.len() == 1 {
                format!("Neue E-Mail von {from}")
            } else {
                format!("{} neue E-Mails", new_unread.len())
            };
            self.host.notify(&title, &first.subject);
        }
        Ok(result)
    }

    pub(crate) fn is_inbox(&self, mailbox_id: &str, folder_id: &str) -> bool {
        self.store
            .get_folder(mailbox_id, folder_id)
            .ok()
            .flatten()
            .and_then(|f| f.well_known)
            .as_deref()
            == Some("inbox")
    }

    async fn run_delta(
        &self,
        mb: &Mailbox,
        c: &Arc<GraphClient>,
        folder_id: &str,
    ) -> Result<(SyncResult, Vec<MessageSummary>)> {
        // Full (re)sync window: everything cached in it that the server no
        // longer lists was deleted meanwhile (e.g. while the token expired).
        let mut full_since: Option<String> = None;
        let mut url = match self.store.delta_link(&mb.id, folder_id)? {
            Some(l) => l,
            None => {
                let since = (Utc::now() - ChronoDuration::days(INITIAL_SYNC_DAYS))
                    .to_rfc3339_opts(SecondsFormat::Secs, true);
                full_since = Some(since.clone());
                format!(
                    "{}/mailFolders/{}/messages/delta?$select={}&$filter={}",
                    mb.graph_prefix(),
                    seg(folder_id),
                    MESSAGE_SUMMARY_SELECT,
                    enc(&format!("receivedDateTime ge {since}"))
                )
            }
        };
        let headers = [("Prefer", format!("odata.maxpagesize={PAGE_SIZE}"))];
        let mut result = SyncResult::default();
        let mut new_unread = Vec::new();
        let mut seen = std::collections::HashSet::new();
        loop {
            let page = c.get_h(&url, &headers).await?;
            let mut upserts = Vec::new();
            for item in page["value"].as_array().cloned().unwrap_or_default() {
                let id = item["id"].as_str().unwrap_or("").to_string();
                if id.is_empty() {
                    continue;
                }
                if item.get("@removed").is_some() {
                    if let Some(m) = self.store.get_summary(&mb.id, &id)? {
                        if m.folder_id == folder_id {
                            self.store.delete_message(&mb.id, &id)?;
                            result.removed += 1;
                        }
                    }
                    continue;
                }
                seen.insert(id.clone());
                let cached = self.store.get_summary(&mb.id, &id)?;
                let mut m = if convert::is_partial_message(&item) {
                    // Only the changed properties: merge into the cached row, or
                    // fetch the whole message if we have never seen it.
                    match &cached {
                        Some(old) => convert::merge_message_summary(old, &item),
                        None => match self.fetch_summary(mb, c, &id).await? {
                            Some(full) => full,
                            None => continue,
                        },
                    }
                } else {
                    convert::message_summary(&item, &mb.id, folder_id)
                };
                m.folder_id = folder_id.to_string();
                match cached {
                    Some(old) => {
                        if old != m {
                            result.updated += 1;
                            if old.is_read != m.is_read || old.categories != m.categories || old.flag != m.flag {
                                // keep cached body in sync with flags
                                if let Some(mut d) = self.store.get_detail(&mb.id, &id)? {
                                    d.summary = m.clone();
                                    self.store.put_detail(&d)?;
                                }
                            }
                        }
                    }
                    None => {
                        result.added += 1;
                        if !m.is_read && !m.is_draft {
                            new_unread.push(m.clone());
                        }
                    }
                }
                upserts.push(m);
            }
            self.store.upsert_messages(&upserts)?;
            if let Some(next) = page["@odata.nextLink"].as_str() {
                url = next.to_string();
                continue;
            }
            if let Some(delta) = page["@odata.deltaLink"].as_str() {
                if let Some(since) = &full_since {
                    for id in self.store.message_ids_since(&mb.id, folder_id, since)? {
                        if !seen.contains(&id) {
                            self.store.delete_message(&mb.id, &id)?;
                            result.removed += 1;
                        }
                    }
                }
                self.store.set_delta_link(&mb.id, folder_id, Some(delta))?;
            }
            break;
        }
        Ok((result, new_unread))
    }

    /// One message summary straight from the server (None: it is gone).
    async fn fetch_summary(&self, mb: &Mailbox, c: &Arc<GraphClient>, id: &str) -> Result<Option<MessageSummary>> {
        match c
            .get(&format!("{}/messages/{}?$select={}", mb.graph_prefix(), seg(id), MESSAGE_SUMMARY_SELECT))
            .await
        {
            Ok(v) => Ok(Some(convert::message_summary(&v, &mb.id, ""))),
            Err(Error::Graph { status: 404, .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Re-fetches cached rows that an incomplete server answer left without
    /// sender, subject and date (shown as "(Unbekannt)" at the end of the list).
    async fn repair_incomplete(&self, mb: &Mailbox, c: &Arc<GraphClient>, folder_id: &str) -> Result<usize> {
        let ids = self.store.incomplete_message_ids(&mb.id, folder_id, 50)?;
        let mut fixed = 0;
        for id in ids {
            match self.fetch_summary(mb, c, &id).await? {
                Some(mut m) if !m.received_at.is_empty() => {
                    m.folder_id = folder_id.to_string();
                    self.store.upsert_messages(&[m])?;
                    fixed += 1;
                }
                Some(_) => {}
                None => {
                    self.store.delete_message(&mb.id, &id)?;
                    fixed += 1;
                }
            }
        }
        Ok(fixed)
    }

    async fn refresh_folder_counts(&self, mb: &Mailbox, c: &Arc<GraphClient>, folder_id: &str) {
        if let Ok(v) = c
            .get(&format!(
                "{}/mailFolders/{}?$select=id,unreadItemCount,totalItemCount,childFolderCount",
                mb.graph_prefix(),
                seg(folder_id)
            ))
            .await
        {
            if let Ok(Some(mut f)) = self.store.get_folder(&mb.id, folder_id) {
                f.unread_count = v["unreadItemCount"].as_i64().unwrap_or(f.unread_count);
                f.total_count = v["totalItemCount"].as_i64().unwrap_or(f.total_count);
                let _ = self.store.upsert_folder(&f);
            }
        }
    }

    pub(crate) async fn load_older(&self, mailbox_id: &str, folder_id: &str) -> Result<i64> {
        if crate::imap::is_imap_id(mailbox_id) {
            return self.imap_load_older(mailbox_id, folder_id).await;
        }
        let lock = self.folder_lock(mailbox_id, folder_id);
        let _g = lock.lock().await;
        let (mb, c) = self.mailbox(mailbox_id)?;
        let mut path = format!(
            "{}/mailFolders/{}/messages?$select={}&$orderby={}&$top={PAGE_SIZE}",
            mb.graph_prefix(),
            seg(folder_id),
            MESSAGE_SUMMARY_SELECT,
            enc("receivedDateTime desc"),
        );
        if let Some(oldest) = self.store.oldest_received(mailbox_id, folder_id)? {
            path.push_str(&format!("&$filter={}", enc(&format!("receivedDateTime lt {oldest}"))));
        }
        let page = c.get(&path).await?;
        let items: Vec<MessageSummary> = page["value"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|v| {
                        let mut m = convert::message_summary(v, mailbox_id, folder_id);
                        m.folder_id = folder_id.to_string();
                        m
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.store.upsert_messages(&items)?;
        if items.len() < PAGE_SIZE || page["@odata.nextLink"].is_null() {
            self.store.set_fully_loaded(mailbox_id, folder_id, true)?;
        }
        if !items.is_empty() {
            self.emit_mail_changed(mailbox_id, Some(folder_id));
        }
        Ok(items.len() as i64)
    }

    pub(crate) async fn search_messages(
        &self,
        mailbox_id: &str,
        text: &str,
        folder_id: Option<&str>,
        field: &str,
        has_attachments: bool,
    ) -> Result<Vec<MessageSummary>> {
        if crate::imap::is_imap_id(mailbox_id) {
            return self.imap_search(mailbox_id, text, folder_id, field, has_attachments).await;
        }
        let text = text.trim();
        if text.is_empty() {
            return Ok(vec![]);
        }
        let (mb, c) = self.mailbox(mailbox_id)?;
        let base = match folder_id {
            Some(f) => format!("{}/mailFolders/{}/messages", mb.graph_prefix(), seg(f)),
            None => format!("{}/messages", mb.graph_prefix()),
        };
        let term = format!("\"{}\"", kql(text, field, has_attachments));
        let path = format!("{base}?$search={}&$select={}&$top=100", enc(&term), MESSAGE_SUMMARY_SELECT);
        let page = c.get(&path).await?;
        let mut out: Vec<MessageSummary> = page["value"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|v| convert::message_summary(v, mailbox_id, folder_id.unwrap_or("")))
                    .collect()
            })
            .unwrap_or_default();
        out.sort_by(|a, b| b.received_at.cmp(&a.received_at));
        Ok(out)
    }

    // ---- single message ----

    pub(crate) async fn get_message(&self, mailbox_id: &str, message_id: &str) -> Result<MessageDetail> {
        if crate::imap::is_imap_id(mailbox_id) {
            return self.imap_get_message(mailbox_id, message_id).await;
        }
        let (mb, c) = match self.mailbox(mailbox_id) {
            Ok(x) => x,
            Err(e) => {
                return self.cached_detail(mailbox_id, message_id).ok_or(e);
            }
        };
        let fetched = self.fetch_detail(&mb, &c, message_id).await;
        match fetched {
            Ok(d) => {
                self.store.put_detail(&d)?;
                if self.store.message_exists(mailbox_id, message_id)? {
                    self.store.upsert_messages(std::slice::from_ref(&d.summary))?;
                }
                Ok(d)
            }
            Err(e) if e.is_network() => self.cached_detail(mailbox_id, message_id).ok_or(e),
            Err(e) => Err(e),
        }
    }

    fn cached_detail(&self, mailbox_id: &str, message_id: &str) -> Option<MessageDetail> {
        let mut d = self.store.get_detail(mailbox_id, message_id).ok()??;
        if let Ok(Some(s)) = self.store.get_summary(mailbox_id, message_id) {
            d.summary = s;
        }
        Some(d)
    }

    pub(crate) async fn fetch_detail(&self, mb: &Mailbox, c: &Arc<GraphClient>, message_id: &str) -> Result<MessageDetail> {
        let prefix = mb.graph_prefix();
        let v = c
            .get(&format!(
                "{prefix}/messages/{}?$select={MESSAGE_DETAIL_SELECT}",
                seg(message_id)
            ))
            .await?;
        let summary = convert::message_summary(&v, &mb.id, "");
        let mut body = convert::body_html(&v);

        let odata_type = v["@odata.type"].as_str().unwrap_or("");
        let is_meeting = odata_type.ends_with("eventMessageRequest")
            || (odata_type.contains("eventMessage") && v["meetingMessageType"].as_str() == Some("meetingRequest"));
        let mut event_id = None;
        if is_meeting {
            if let Ok(ev) = c
                .get(&format!(
                    "{prefix}/messages/{}?$select=id&$expand={}",
                    seg(message_id),
                    enc("microsoft.graph.eventMessage/event($select=id)")
                ))
                .await
            {
                event_id = ev["event"]["id"].as_str().map(str::to_string);
            }
        }

        let mut attachments = Vec::new();
        let references_cid = body.contains("cid:");
        if summary.has_attachments || references_cid {
            let list = c
                .get_all(
                    &format!(
                        "{prefix}/messages/{}/attachments?$select=id,name,contentType,size,isInline",
                        seg(message_id)
                    ),
                    &[],
                    500,
                )
                .await?;
            for a in list {
                let att = convert::attachment(&a);
                if att.is_inline && references_cid {
                    // Fetch content and substitute cid: references.
                    if let Ok(full) = c
                        .get(&format!(
                            "{prefix}/messages/{}/attachments/{}",
                            seg(message_id),
                            seg(&att.id)
                        ))
                        .await
                    {
                        let cid = full["contentId"].as_str().unwrap_or("").trim_matches(['<', '>']);
                        let bytes = full["contentBytes"].as_str().unwrap_or("");
                        if !cid.is_empty() && !bytes.is_empty() {
                            let needle = format!("cid:{cid}");
                            if body.contains(&needle) {
                                let data = format!("data:{};base64,{}", att.content_type, bytes);
                                body = body.replace(&needle, &data);
                                continue;
                            }
                        }
                    }
                }
                if att.is_inline && att.content_type.starts_with("image/") && references_cid {
                    continue;
                }
                attachments.push(att);
            }
        }

        Ok(MessageDetail {
            summary,
            cc: convert::addresses(&v, "ccRecipients"),
            bcc: convert::addresses(&v, "bccRecipients"),
            reply_to: convert::addresses(&v, "replyTo"),
            sent_at: v["sentDateTime"].as_str().map(convert::normalize_utc),
            body_html: body,
            attachments,
            internet_message_id: v["internetMessageId"].as_str().map(str::to_string),
            web_link: v["webLink"].as_str().map(str::to_string),
            is_meeting_request: is_meeting,
            event_id,
        })
    }

    pub(crate) async fn get_message_source(&self, mailbox_id: &str, message_id: &str) -> Result<String> {
        if crate::imap::is_imap_id(mailbox_id) {
            return self.imap_message_source(mailbox_id, message_id).await;
        }
        let (mb, c) = self.mailbox(mailbox_id)?;
        let bytes = c
            .get_bytes(&format!("{}/messages/{}/$value", mb.graph_prefix(), seg(message_id)))
            .await?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    // ---- bulk updates ----

    pub(crate) async fn patch_many(&self, mb: &Mailbox, c: &Arc<GraphClient>, ids: &[String], body: Value) -> Result<()> {
        let prefix = mb.graph_prefix();
        let gone: Vec<Option<String>> = stream::iter(ids.to_vec())
            .map(|id| {
                let c = c.clone();
                let body = body.clone();
                let p = format!("{prefix}/messages/{}", seg(&id));
                async move {
                    match c.patch(&p, &body).await {
                        Ok(_) => Ok(None),
                        Err(e) if e.is_not_found() => Ok(Some(id)),
                        Err(e) => Err(e),
                    }
                }
            })
            .buffer_unordered(PARALLEL)
            .try_collect::<Vec<_>>()
            .await?;
        // Messages deleted on the server meanwhile leave the cache instead of failing.
        let gone: Vec<String> = gone.into_iter().flatten().collect();
        for id in &gone {
            if let Some(old) = self.store.get_summary(&mb.id, id)? {
                self.store
                    .adjust_folder_counts(&mb.id, &old.folder_id, if old.is_read { 0 } else { -1 }, -1)?;
            }
            self.store.delete_message(&mb.id, id)?;
        }
        if !gone.is_empty() {
            self.emit_mail_changed(&mb.id, None);
        }
        Ok(())
    }

    pub(crate) fn update_cached(&self, mailbox_id: &str, ids: &[String], f: impl Fn(&mut MessageSummary)) -> Result<HashSet<String>> {
        let mut folders = HashSet::new();
        for id in ids {
            if let Some(mut m) = self.store.get_summary(mailbox_id, id)? {
                let was_unread = !m.is_read;
                f(&mut m);
                if was_unread != !m.is_read {
                    self.store
                        .adjust_folder_counts(mailbox_id, &m.folder_id, if m.is_read { -1 } else { 1 }, 0)?;
                }
                folders.insert(m.folder_id.clone());
                self.store.upsert_messages(&[m.clone()])?;
                if let Some(mut d) = self.store.get_detail(mailbox_id, id)? {
                    d.summary.is_read = m.is_read;
                    d.summary.flag = m.flag.clone();
                    d.summary.categories = m.categories.clone();
                    self.store.put_detail(&d)?;
                }
            }
        }
        Ok(folders)
    }

    fn emit_folders(&self, mailbox_id: &str, folders: HashSet<String>) {
        if folders.is_empty() {
            self.emit_mail_changed(mailbox_id, None);
        }
        for f in folders {
            self.emit_mail_changed(mailbox_id, Some(&f));
        }
    }

    pub(crate) async fn set_read(&self, mailbox_id: &str, ids: &[String], is_read: bool) -> Result<()> {
        if crate::imap::is_imap_id(mailbox_id) {
            return self.imap_set_read(mailbox_id, ids, is_read).await;
        }
        let (mb, c) = self.mailbox(mailbox_id)?;
        self.patch_many(&mb, &c, ids, json!({"isRead": is_read})).await?;
        let folders = self.update_cached(mailbox_id, ids, |m| m.is_read = is_read)?;
        self.emit_folders(mailbox_id, folders);
        Ok(())
    }

    pub(crate) async fn set_flag(&self, mailbox_id: &str, ids: &[String], flag: &str) -> Result<()> {
        if crate::imap::is_imap_id(mailbox_id) {
            return self.imap_set_flag(mailbox_id, ids, flag).await;
        }
        if !matches!(flag, "notFlagged" | "flagged" | "complete") {
            return Err(Error::invalid("Ungültige Kennzeichnung"));
        }
        let (mb, c) = self.mailbox(mailbox_id)?;
        self.patch_many(&mb, &c, ids, json!({"flag": {"flagStatus": flag}}))
            .await?;
        let folders = self.update_cached(mailbox_id, ids, |m| m.flag = flag.to_string())?;
        self.emit_folders(mailbox_id, folders);
        Ok(())
    }

    pub(crate) async fn set_categories(&self, mailbox_id: &str, ids: &[String], categories: &[String]) -> Result<()> {
        if crate::imap::is_imap_id(mailbox_id) {
            return self.imap_set_categories(mailbox_id, ids, categories).await;
        }
        let (mb, c) = self.mailbox(mailbox_id)?;
        self.patch_many(&mb, &c, ids, json!({"categories": categories}))
            .await?;
        let folders = self.update_cached(mailbox_id, ids, |m| m.categories = categories.to_vec())?;
        self.emit_folders(mailbox_id, folders);
        Ok(())
    }

    /// Moves/copies messages; returns `(old id, new id)` pairs (Graph assigns new ids).
    pub(crate) async fn move_messages(
        &self,
        mailbox_id: &str,
        ids: &[String],
        dest: &str,
        copy: bool,
    ) -> Result<Vec<(String, String)>> {
        if crate::imap::is_imap_id(mailbox_id) {
            return self.imap_move(mailbox_id, ids, dest, copy).await;
        }
        let (mb, c) = self.mailbox(mailbox_id)?;
        let prefix = mb.graph_prefix();
        let action = if copy { "copy" } else { "move" };
        let results: Vec<(String, Value)> = stream::iter(ids.to_vec())
            .map(|id| {
                let c = c.clone();
                let p = format!("{prefix}/messages/{}/{action}", seg(&id));
                let body = json!({"destinationId": dest});
                async move {
                    match c.post(&p, &body).await {
                        Ok(v) => Ok((id, v)),
                        // Gone on the server already (e.g. a meeting response the
                        // calendar attendant processed and deleted): drop it locally.
                        Err(e) if e.is_not_found() => Ok((id, Value::Null)),
                        Err(e) => Err(e),
                    }
                }
            })
            .buffer_unordered(PARALLEL)
            .try_collect()
            .await?;
        let mut folders = HashSet::from([dest.to_string()]);
        let mut mapping = Vec::new();
        for (old_id, v) in results {
            if let Some(new_id) = v["id"].as_str() {
                mapping.push((old_id.clone(), new_id.to_string()));
            }
            if !copy {
                if let Some(old) = self.store.get_summary(mailbox_id, &old_id)? {
                    self.store.adjust_folder_counts(
                        mailbox_id,
                        &old.folder_id,
                        if old.is_read { 0 } else { -1 },
                        -1,
                    )?;
                    folders.insert(old.folder_id);
                }
                self.store.delete_message(mailbox_id, &old_id)?;
            }
            if v.is_object() && v["id"].is_string() {
                let mut m = convert::message_summary(&v, mailbox_id, dest);
                m.folder_id = dest.to_string();
                self.store
                    .adjust_folder_counts(mailbox_id, dest, if m.is_read { 0 } else { 1 }, 1)?;
                self.store.upsert_messages(&[m])?;
            }
        }
        self.emit_folders(mailbox_id, folders);
        Ok(mapping)
    }

    pub(crate) async fn delete_messages(&self, mailbox_id: &str, ids: &[String], permanent: bool) -> Result<()> {
        if crate::imap::is_imap_id(mailbox_id) {
            return self.imap_delete(mailbox_id, ids, permanent).await;
        }
        let deleted = self.store.folder_by_well_known(mailbox_id, "deleteditems")?;
        let (mut to_move, mut to_delete) = (Vec::new(), Vec::new());
        for id in ids {
            let in_deleted = match (&deleted, self.store.get_summary(mailbox_id, id)?) {
                (Some(d), Some(m)) => m.folder_id == d.id,
                _ => false,
            };
            if permanent || in_deleted || deleted.is_none() {
                to_delete.push(id.clone());
            } else {
                to_move.push(id.clone());
            }
        }
        if let (Some(d), false) = (&deleted, to_move.is_empty()) {
            self.move_messages(mailbox_id, &to_move, &d.id, false).await?;
        }
        if !to_delete.is_empty() {
            let (mb, c) = self.mailbox(mailbox_id)?;
            let prefix = mb.graph_prefix();
            stream::iter(to_delete.clone())
                .map(|id| {
                    let c = c.clone();
                    let p = format!("{prefix}/messages/{}", seg(&id));
                    async move {
                        match c.delete(&p).await {
                            Err(e) if e.is_not_found() => Ok(()),
                            other => other,
                        }
                    }
                })
                .buffer_unordered(PARALLEL)
                .try_collect::<Vec<_>>()
                .await?;
            let mut folders = HashSet::new();
            for id in &to_delete {
                if let Some(old) = self.store.get_summary(mailbox_id, id)? {
                    self.store.adjust_folder_counts(
                        mailbox_id,
                        &old.folder_id,
                        if old.is_read { 0 } else { -1 },
                        -1,
                    )?;
                    folders.insert(old.folder_id);
                }
                self.store.delete_message(mailbox_id, id)?;
            }
            self.emit_folders(mailbox_id, folders);
        }
        Ok(())
    }

    pub(crate) async fn download_attachment(
        &self,
        mailbox_id: &str,
        message_id: &str,
        attachment_id: &str,
        action: &str,
    ) -> Result<Option<String>> {
        if crate::imap::is_imap_id(mailbox_id) {
            return self.imap_download_attachment(mailbox_id, message_id, attachment_id, action).await;
        }
        let (mb, c) = self.mailbox(mailbox_id)?;
        let prefix = mb.graph_prefix();
        let meta = c
            .get(&format!(
                "{prefix}/messages/{}/attachments/{}?$select=id,name,contentType,size",
                seg(message_id),
                seg(attachment_id)
            ))
            .await?;
        let name = sanitize_filename(meta["name"].as_str().unwrap_or("anhang"));
        let bytes = match c
            .get_bytes(&format!(
                "{prefix}/messages/{}/attachments/{}/$value",
                seg(message_id),
                seg(attachment_id)
            ))
            .await
        {
            Ok(b) => b,
            Err(_) => {
                // Fallback for attachment types without $value support.
                let full = c
                    .get(&format!(
                        "{prefix}/messages/{}/attachments/{}",
                        seg(message_id),
                        seg(attachment_id)
                    ))
                    .await?;
                base64::engine::general_purpose::STANDARD
                    .decode(full["contentBytes"].as_str().unwrap_or(""))
                    .map_err(|e| Error::Other(e.to_string()))?
            }
        };
        let path = if action == "save" {
            match self.host.save_dialog(&name).await {
                Some(p) => p,
                None => return Ok(None),
            }
        } else {
            let dir = std::env::temp_dir()
                .join("lettertask")
                .join(uuid::Uuid::new_v4().to_string());
            std::fs::create_dir_all(&dir)?;
            dir.join(&name)
        };
        std::fs::write(&path, bytes)?;
        if action != "save" {
            self.host.open_path(&path)?;
        }
        Ok(Some(path.to_string_lossy().into_owned()))
    }
}

/// Builds the KQL expression for `$search` (without the surrounding quotes,
/// which Graph requires around the whole value).
pub(crate) fn kql(text: &str, field: &str, has_attachments: bool) -> String {
    let clean: String = text
        .chars()
        .map(|c| if matches!(c, '"' | '\\' | '(' | ')') { ' ' } else { c })
        .collect();
    let words: Vec<&str> = clean.split_whitespace().collect();
    let prop = match field {
        "from" => Some("from"),
        "to" => Some("to"),
        "subject" => Some("subject"),
        "body" => Some("body"),
        _ => None,
    };
    let mut q = match prop {
        Some(p) => words.iter().map(|w| format!("{p}:{w}")).collect::<Vec<_>>().join(" "),
        None => words.join(" "),
    };
    if has_attachments {
        q.push_str(" hasAttachments:true");
    }
    q.trim().to_string()
}

pub(crate) use crate::files::sanitize_filename;

#[cfg(test)]
mod tests {
    use super::kql;

    #[test]
    fn builds_kql() {
        assert_eq!(kql("Rechnung März", "all", false), "Rechnung März");
        assert_eq!(kql("anna \"x\"", "from", false), "from:anna from:x");
        assert_eq!(kql("Angebot", "subject", true), "subject:Angebot hasAttachments:true");
    }
}
