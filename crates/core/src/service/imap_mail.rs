//! Mail operations for IMAP accounts (folders, sync, flags, categories as
//! keywords, move/delete, archive, Wiedervorlage, search, attachments).

use std::collections::{HashMap, HashSet};

use async_imap::types::{Flag, NameAttribute};
use chrono::{DateTime, Utc};
use futures::TryStreamExt;
use serde_json::json;

use super::imap_account::with_imap;
use super::Service;
use crate::error::{Error, Result};
use crate::imap::conn::{timed, Conn, OP_TIMEOUT};
use crate::imap::{self as im, mime};
use crate::model::*;

/// Messages fetched by the first sync of a folder; older ones load on demand.
const INITIAL_FETCH: usize = 500;
const OLDER_FETCH: usize = 200;
const PREVIEW_BYTES: usize = 2048;
pub(crate) const SNOOZE_FOLDER: &str = "Wiedervorlage";
const ARCHIVE_FOLDER: &str = "Archive";

fn summary_query() -> String {
    format!("(UID FLAGS INTERNALDATE RFC822.SIZE BODY.PEEK[HEADER] BODY.PEEK[TEXT]<0.{PREVIEW_BYTES}>)")
}

/// Owned copy of the parts of a FETCH response we use.
pub(crate) struct Fetched {
    pub uid: u32,
    pub flags: Vec<String>,
    pub internal_date: Option<DateTime<chrono::FixedOffset>>,
    pub header: Option<Vec<u8>>,
    pub text: Option<Vec<u8>>,
    pub body: Option<Vec<u8>>,
}

pub(crate) async fn uid_fetch(conn: &mut Conn, set: &str, query: &str) -> Result<Vec<Fetched>> {
    let stream = timed(conn.s().uid_fetch(set, query)).await?;
    let items: Vec<async_imap::types::Fetch> = tokio::time::timeout(OP_TIMEOUT, stream.try_collect())
        .await
        .map_err(|_| Error::Network("Zeitüberschreitung beim Abrufen".into()))?
        .map_err(crate::imap::conn::imap_err)?;
    Ok(items
        .into_iter()
        .filter_map(|f| {
            Some(Fetched {
                uid: f.uid?,
                flags: mime::flag_strings(f.flags()),
                internal_date: f.internal_date(),
                header: f.header().map(<[u8]>::to_vec),
                text: f.text().map(<[u8]>::to_vec),
                body: f.body().map(<[u8]>::to_vec),
            })
        })
        .collect())
}

pub(crate) async fn uid_search(conn: &mut Conn, query: &str) -> Result<Vec<u32>> {
    let mut v: Vec<u32> = timed(conn.s().uid_search(query)).await?.into_iter().collect();
    v.sort_unstable();
    Ok(v)
}

async fn store_flags(conn: &mut Conn, set: &str, op: &str) -> Result<()> {
    let stream = timed(conn.s().uid_store(set, op)).await?;
    let _: Vec<_> = tokio::time::timeout(OP_TIMEOUT, stream.try_collect())
        .await
        .map_err(|_| Error::Network("Zeitüberschreitung".into()))?
        .map_err(crate::imap::conn::imap_err)?;
    Ok(())
}

/// Permanently removes `uids` from the selected folder.
pub(crate) async fn expunge_uids(conn: &mut Conn, uids: &[u32]) -> Result<()> {
    if uids.is_empty() {
        return Ok(());
    }
    let set = im::uid_set(uids);
    store_flags(conn, &set, "+FLAGS.SILENT (\\Deleted)").await?;
    if conn.uidplus {
        let stream = timed(conn.s().uid_expunge(&set)).await?;
        let _: Vec<_> = stream.try_collect().await.map_err(crate::imap::conn::imap_err)?;
    } else {
        let stream = timed(conn.s().expunge()).await?;
        let _: Vec<_> = stream.try_collect().await.map_err(crate::imap::conn::imap_err)?;
    }
    Ok(())
}

/// Moves (or copies) `uids` of the selected folder to `dest`.
async fn transfer(conn: &mut Conn, uids: &[u32], dest: &str, copy: bool) -> Result<()> {
    let set = im::uid_set(uids);
    if copy {
        timed(conn.s().uid_copy(&set, dest)).await?;
    } else if conn.can_move {
        timed(conn.s().uid_mv(&set, dest)).await?;
    } else {
        timed(conn.s().uid_copy(&set, dest)).await?;
        expunge_uids(conn, uids).await?;
    }
    Ok(())
}

fn may_create_keywords(mb: &async_imap::types::Mailbox) -> bool {
    mb.permanent_flags.iter().any(|f| matches!(f, Flag::MayCreate))
}

/// Groups message ids by folder: folder → (uidvalidity, uids).
fn by_folder(ids: &[String]) -> Result<HashMap<String, (u32, Vec<u32>)>> {
    let mut map: HashMap<String, (u32, Vec<u32>)> = HashMap::new();
    for id in ids {
        let (v, uid, folder) = im::parse_message_id(id).ok_or_else(|| Error::invalid(format!("Ungültige Nachrichten-ID: {id}")))?;
        map.entry(folder.to_string()).or_insert((v, vec![])).1.push(uid);
    }
    Ok(map)
}

fn uv_key(mb: &str, folder: &str) -> String {
    format!("imapuv|{mb}|{folder}")
}

fn delim_key(mb: &str) -> String {
    format!("imapdelim|{mb}")
}

pub(crate) fn snooze_key(mb: &str, message_id: &str) -> String {
    format!("imapsnooze|{mb}|{message_id}")
}

/// Fallback mapping of folder names to roles when the server lacks SPECIAL-USE.
fn well_known_by_name(display: &str) -> Option<&'static str> {
    let n = display.to_lowercase();
    Some(match n.as_str() {
        "sent" | "gesendet" | "gesendete elemente" | "sent items" | "sent messages" | "gesendete objekte" => "sentitems",
        "drafts" | "entwürfe" | "draft" => "drafts",
        "trash" | "papierkorb" | "deleted items" | "deleted messages" | "gelöschte elemente" | "gelöschte objekte" => "deleteditems",
        "junk" | "spam" | "junk-e-mail" | "junk e-mail" | "bulk mail" => "junkemail",
        "archive" | "archiv" => "archive",
        _ => return None,
    })
}

impl Service {
    fn imap_delim(&self, mb: &str) -> String {
        self.store.kv_get(&delim_key(mb)).ok().flatten().unwrap_or_else(|| "/".into())
    }

    // ---- folders ----

    pub(crate) async fn imap_sync_folders(&self, mb: &str) -> Result<Vec<Folder>> {
        struct Raw {
            name: String,
            delim: Option<String>,
            special: Option<&'static str>,
            selectable: bool,
        }
        let raws: Vec<(Raw, Option<(u32, u32)>)> = with_imap!(self, mb, |c| {
                let names: Vec<async_imap::types::Name> = {
                    let stream = timed(c.s().list(Some(""), Some("*"))).await?;
                    stream.try_collect().await.map_err(crate::imap::conn::imap_err)?
                };
                let mut out = Vec::new();
                for n in &names {
                    let attrs = n.attributes();
                    let special = attrs.iter().find_map(|a| match a {
                        NameAttribute::Sent => Some("sentitems"),
                        NameAttribute::Drafts => Some("drafts"),
                        NameAttribute::Trash => Some("deleteditems"),
                        NameAttribute::Junk => Some("junkemail"),
                        NameAttribute::Archive => Some("archive"),
                        _ => None,
                    });
                    let selectable = !attrs
                        .iter()
                        .any(|a| matches!(a, NameAttribute::NoSelect) || matches!(a, NameAttribute::Extension(x) if x.eq_ignore_ascii_case("\\NonExistent")));
                    out.push(Raw {
                        name: n.name().to_string(),
                        delim: n.delimiter().map(str::to_string),
                        special,
                        selectable,
                    });
                }
                let mut with_status = Vec::new();
                for r in out {
                    let status = if r.selectable {
                        timed(c.s().status(&r.name, "(MESSAGES UNSEEN)"))
                            .await
                            .ok()
                            .map(|s| (s.exists, s.unseen.unwrap_or(0)))
                    } else {
                        None
                    };
                    with_status.push((r, status));
                }
                Ok(with_status)
            })?;

        if let Some(d) = raws.iter().find_map(|(r, _)| r.delim.clone()) {
            self.store.kv_set(&delim_key(mb), &d)?;
        }
        let selectable: HashSet<&str> = raws.iter().filter(|(r, _)| r.selectable).map(|(r, _)| r.name.as_str()).collect();
        let mut used: HashSet<&'static str> = raws.iter().filter_map(|(r, _)| r.special).collect();
        let mut folders = Vec::new();
        for (r, status) in &raws {
            if !r.selectable {
                continue;
            }
            let (parent, leaf) = match &r.delim {
                Some(d) if !d.is_empty() => match r.name.rsplit_once(d.as_str()) {
                    Some((p, l)) => {
                        // Attach to the nearest selectable ancestor.
                        let mut p = p.to_string();
                        loop {
                            if selectable.contains(p.as_str()) {
                                break (Some(p), l.to_string());
                            }
                            match p.rsplit_once(d.as_str()) {
                                Some((pp, _)) => p = pp.to_string(),
                                None => break (None, l.to_string()),
                            }
                        }
                    }
                    None => (None, r.name.clone()),
                },
                _ => (None, r.name.clone()),
            };
            let display = utf7_imap::decode_utf7_imap(leaf);
            let mut well_known = if r.name.eq_ignore_ascii_case("INBOX") {
                Some("inbox".to_string())
            } else {
                r.special.map(str::to_string)
            };
            if well_known.is_none() {
                if let Some(w) = well_known_by_name(&display) {
                    if !used.contains(w) {
                        used.insert(w);
                        well_known = Some(w.to_string());
                    }
                }
            }
            let (total, unread) = status.unwrap_or((0, 0));
            let mut f = Folder {
                id: r.name.clone(),
                mailbox_id: mb.to_string(),
                parent_id: parent,
                display_name: if r.name.eq_ignore_ascii_case("INBOX") { "Posteingang".into() } else { display },
                well_known,
                unread_count: unread as i64,
                total_count: total as i64,
                child_folder_count: 0,
            };
            if f.well_known.is_none() && self.is_snooze_folder(mb, &f)? {
                f.well_known = Some("snoozed".into());
            }
            folders.push(f);
        }
        let counts: HashMap<String, i64> = folders.iter().filter_map(|f| f.parent_id.clone()).fold(HashMap::new(), |mut m, p| {
            *m.entry(p).or_default() += 1;
            m
        });
        for f in &mut folders {
            f.child_folder_count = counts.get(&f.id).copied().unwrap_or(0);
        }
        self.store.replace_folders(mb, &folders)?;
        self.store.list_folders(mb)
    }

    fn imap_child_name(&self, mb: &str, parent: Option<&str>, name: &str) -> Result<String> {
        let name = name.trim();
        if name.is_empty() {
            return Err(Error::invalid("Ordnername darf nicht leer sein."));
        }
        let delim = self.imap_delim(mb);
        if name.contains(&delim) {
            return Err(Error::invalid(format!("Der Ordnername darf kein \"{delim}\" enthalten.")));
        }
        let enc = utf7_imap::encode_utf7_imap(name.to_string());
        Ok(match parent {
            Some(p) => format!("{p}{delim}{enc}"),
            None => enc,
        })
    }

    pub(crate) async fn imap_create_folder(&self, mb: &str, parent: Option<&str>, name: &str) -> Result<Folder> {
        let full = self.imap_child_name(mb, parent, name)?;
        with_imap!(self, mb, |c| {
            timed(c.s().create(&full)).await?;
            let _ = timed(c.s().subscribe(&full)).await;
            Ok(())
        })?;
        self.imap_sync_folders(mb).await?;
        self.emit_mail_changed(mb, None);
        self.store
            .get_folder(mb, &full)?
            .ok_or_else(|| Error::NotFound("Ordner".into()))
    }

    pub(crate) async fn imap_rename_folder(&self, mb: &str, folder: &str, name: &str) -> Result<Folder> {
        let f = self.store.get_folder(mb, folder)?.ok_or_else(|| Error::NotFound("Ordner".into()))?;
        let full = self.imap_child_name(mb, f.parent_id.as_deref(), name)?;
        with_imap!(self, mb, |c| {
            c.selected = None;
            timed(c.s().rename(folder, &full)).await?;
            let _ = timed(c.s().subscribe(&full)).await;
            Ok(())
        })?;
        self.imap_sync_folders(mb).await?;
        self.emit_mail_changed(mb, None);
        self.store
            .get_folder(mb, &full)?
            .ok_or_else(|| Error::NotFound("Ordner".into()))
    }

    pub(crate) async fn imap_delete_folder(&self, mb: &str, folder: &str) -> Result<()> {
        if let Some(f) = self.store.get_folder(mb, folder)? {
            if f.well_known.is_some() {
                return Err(Error::invalid("Standardordner können nicht gelöscht werden."));
            }
        }
        with_imap!(self, mb, |c| {
            c.selected = None;
            let _ = timed(c.s().unsubscribe(folder)).await;
            timed(c.s().delete(folder)).await
        })?;
        self.store.delete_folder(mb, folder)?;
        self.imap_sync_folders(mb).await?;
        self.emit_mail_changed(mb, None);
        Ok(())
    }

    pub(crate) async fn imap_mark_folder_read(&self, mb: &str, folder: &str) -> Result<()> {
        with_imap!(self, mb, |c| {
            c.select(folder).await?;
            let unseen = uid_search(c, "UNSEEN").await?;
            if !unseen.is_empty() {
                store_flags(c, &im::uid_set(&unseen), "+FLAGS.SILENT (\\Seen)").await?;
            }
            Ok(())
        })?;
        let ids = self.store.folder_message_ids(mb, folder, true)?;
        self.update_cached(mb, &ids, |m| m.is_read = true)?;
        if let Some(mut f) = self.store.get_folder(mb, folder)? {
            f.unread_count = 0;
            self.store.upsert_folder(&f)?;
        }
        self.emit_mail_changed(mb, Some(folder));
        Ok(())
    }

    pub(crate) async fn imap_empty_folder(&self, mb: &str, folder: &str) -> Result<()> {
        with_imap!(self, mb, |c| {
            c.select(folder).await?;
            let all = uid_search(c, "ALL").await?;
            expunge_uids(c, &all).await
        })?;
        self.store.delete_folder_messages(mb, folder)?;
        if let Some(mut f) = self.store.get_folder(mb, folder)? {
            f.unread_count = 0;
            f.total_count = 0;
            self.store.upsert_folder(&f)?;
        }
        self.emit_mail_changed(mb, Some(folder));
        Ok(())
    }

    // ---- sync ----

    fn cached_uids(&self, mb: &str, folder: &str) -> Result<Vec<(u32, String)>> {
        let mut v: Vec<(u32, String)> = self
            .store
            .folder_message_ids(mb, folder, false)?
            .into_iter()
            .filter_map(|id| im::parse_message_id(&id).map(|(_, uid, _)| (uid, id.clone())))
            .collect();
        v.sort_unstable();
        Ok(v)
    }

    fn summaries_from(&self, mb: &str, folder: &str, uv: u32, items: Vec<Fetched>, snoozed: bool) -> Vec<MessageSummary> {
        items
            .into_iter()
            .filter_map(|f| {
                let header = f.header.as_deref().or(f.body.as_deref())?;
                let mut s = mime::summary(
                    mb,
                    folder,
                    im::message_id(uv, f.uid, folder),
                    header,
                    f.text.as_deref(),
                    &f.flags,
                    f.internal_date,
                );
                if snoozed && !s.conversation_id.is_empty() {
                    s.due_at = self.store.kv_get(&snooze_key(mb, &s.conversation_id)).ok().flatten().filter(|d| !d.is_empty());
                }
                Some(s)
            })
            .collect()
    }

    pub(crate) async fn imap_sync_folder(&self, mb: &str, folder: &str, quiet: bool) -> Result<SyncResult> {
        let lock = self.folder_lock(mb, folder);
        let guard = lock.lock().await;
        let snoozed = self.store.get_folder(mb, folder)?.and_then(|f| f.well_known).as_deref() == Some("snoozed");
        let stored_uv: Option<u32> = self.store.kv_get(&uv_key(mb, folder))?.and_then(|v| v.parse().ok());
        let cached_before = self.cached_uids(mb, folder)?;

        struct Outcome {
            uv: u32,
            exists: u32,
            unseen: usize,
            new_items: Vec<Fetched>,
            flags: HashMap<u32, Vec<String>>,
            all_new: bool,
            reset: bool,
        }
        let out = with_imap!(self, mb, |c| {
                let sel = c.select(folder).await?;
                let uv = sel.uid_validity.unwrap_or(0);
                let reset = stored_uv.is_some_and(|s| s != uv);
                let cached: Vec<u32> = if reset { vec![] } else { cached_before.iter().map(|(u, _)| *u).collect() };
                let mut new_uids: Vec<u32> = Vec::new();
                let mut all_new = false;
                if sel.exists > 0 {
                    match cached.last() {
                        None => {
                            let all = uid_search(c, "ALL").await?;
                            all_new = all.len() <= INITIAL_FETCH;
                            new_uids = all.into_iter().rev().take(INITIAL_FETCH).collect();
                        }
                        Some(max) => {
                            new_uids = uid_search(c, &format!("UID {}:*", max + 1))
                                .await?
                                .into_iter()
                                .filter(|u| u > max)
                                .collect();
                        }
                    }
                }
                let new_items = if new_uids.is_empty() {
                    vec![]
                } else {
                    uid_fetch(c, &im::uid_set(&new_uids), &summary_query()).await?
                };
                let mut flags = HashMap::new();
                if let (Some(min), Some(max), true) = (cached.first(), cached.last(), sel.exists > 0) {
                    for f in uid_fetch(c, &format!("{min}:{max}"), "(UID FLAGS)").await? {
                        flags.insert(f.uid, f.flags);
                    }
                }
                let unseen = if sel.exists > 0 { uid_search(c, "UNSEEN").await?.len() } else { 0 };
                Ok(Outcome {
                    uv,
                    exists: sel.exists,
                    unseen,
                    new_items,
                    flags,
                    all_new,
                    reset,
                })
            })?;

        let mut result = SyncResult::default();
        let initial = cached_before.is_empty() || out.reset;
        if out.reset {
            self.store.delete_folder_messages(mb, folder)?;
            self.store.set_fully_loaded(mb, folder, false)?;
        } else {
            // Flag changes and deletions of cached messages.
            let mut changed = Vec::new();
            for (uid, id) in &cached_before {
                match out.flags.get(uid) {
                    None => {
                        self.store.delete_message(mb, id)?;
                        result.removed += 1;
                    }
                    Some(fl) => {
                        if let Some(mut m) = self.store.get_summary(mb, id)? {
                            let (r, f, cats) = (mime::has_flag(fl, "\\Seen"), mime::flag_status(fl), mime::categories(fl));
                            if m.is_read != r || m.flag != f || m.categories != cats {
                                m.is_read = r;
                                m.flag = f;
                                m.categories = cats;
                                changed.push(m);
                            }
                        }
                    }
                }
            }
            result.updated = changed.len() as i64;
            self.store.upsert_messages(&changed)?;
        }
        self.store.kv_set(&uv_key(mb, folder), &out.uv.to_string())?;
        let new = self.summaries_from(mb, folder, out.uv, out.new_items, snoozed);
        result.added = new.len() as i64;
        let mut new_unread: Vec<MessageSummary> = new.iter().filter(|m| !m.is_read && !m.is_draft).cloned().collect();
        self.store.upsert_messages(&new)?;
        if initial && out.all_new {
            self.store.set_fully_loaded(mb, folder, true)?;
        }
        if let Some(mut f) = self.store.get_folder(mb, folder)? {
            f.total_count = out.exists as i64;
            f.unread_count = out.unseen as i64;
            self.store.upsert_folder(&f)?;
        }
        drop(guard);
        let is_inbox = folder.eq_ignore_ascii_case("INBOX");
        if !initial && is_inbox && !new_unread.is_empty() {
            // Rule chain runs before the new mails are announced to the UI.
            let moved = Box::pin(self.apply_inbox_rules(mb, &new_unread)).await;
            new_unread.retain(|m| !moved.contains(&m.id));
        }
        if result.added + result.updated + result.removed > 0 {
            self.emit_mail_changed(mb, Some(folder));
        }
        if !quiet && !initial && is_inbox && !new_unread.is_empty() {
            let first = &new_unread[0];
            let from = first
                .from
                .as_ref()
                .map(|a| if a.name.is_empty() { a.address.clone() } else { a.name.clone() })
                .unwrap_or_default();
            self.emit(
                "new-mail",
                json!({"mailboxId": mb, "count": new_unread.len(), "from": from, "subject": first.subject}),
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

    pub(crate) async fn imap_load_older(&self, mb: &str, folder: &str) -> Result<i64> {
        let lock = self.folder_lock(mb, folder);
        let _g = lock.lock().await;
        let cached = self.cached_uids(mb, folder)?;
        let Some((min, _)) = cached.first().cloned() else {
            drop(_g);
            return self.imap_sync_folder(mb, folder, true).await.map(|r| r.added);
        };
        if min <= 1 {
            self.store.set_fully_loaded(mb, folder, true)?;
            return Ok(0);
        }
        let snoozed = self.store.get_folder(mb, folder)?.and_then(|f| f.well_known).as_deref() == Some("snoozed");
        let (uv, items, more) = with_imap!(self, mb, |c| {
                let sel = c.select(folder).await?;
                let older = uid_search(c, &format!("UID 1:{}", min - 1)).await?;
                let older: Vec<u32> = older.into_iter().filter(|u| *u < min).collect();
                let more = older.len() > OLDER_FETCH;
                let take: Vec<u32> = older.into_iter().rev().take(OLDER_FETCH).collect();
                let items = if take.is_empty() { vec![] } else { uid_fetch(c, &im::uid_set(&take), &summary_query()).await? };
                Ok((sel.uid_validity.unwrap_or(0), items, more))
            })?;
        let list = self.summaries_from(mb, folder, uv, items, snoozed);
        self.store.upsert_messages(&list)?;
        if !more {
            self.store.set_fully_loaded(mb, folder, true)?;
        }
        if !list.is_empty() {
            self.emit_mail_changed(mb, Some(folder));
        }
        Ok(list.len() as i64)
    }

    pub(crate) async fn imap_search(
        &self,
        mb: &str,
        text: &str,
        folder: Option<&str>,
        field: &str,
        has_attachments: bool,
    ) -> Result<Vec<MessageSummary>> {
        let words: Vec<String> = text.split_whitespace().map(str::to_string).collect();
        if words.is_empty() {
            return Ok(vec![]);
        }
        let key = match field {
            "from" => "FROM",
            "to" => "TO",
            "subject" => "SUBJECT",
            "body" => "BODY",
            _ => "TEXT",
        };
        let mut criteria: Vec<String> = words.iter().map(|w| format!("{key} {}", im::quote(w))).collect();
        if has_attachments {
            criteria.push("HEADER Content-Type \"multipart/mixed\"".into());
        }
        let query = format!("CHARSET UTF-8 {}", criteria.join(" "));
        let folders: Vec<String> = match folder {
            Some(f) => vec![f.to_string()],
            None => self
                .store
                .list_folders(mb)?
                .into_iter()
                .filter(|f| f.well_known.as_deref() != Some("junkemail"))
                .map(|f| f.id)
                .collect(),
        };
        let found: Vec<(String, u32, Vec<Fetched>)> = with_imap!(self, mb, |c| {
                let mut out = Vec::new();
                for f in &folders {
                    let sel = c.select(f).await?;
                    let uids = match uid_search(c, &query).await {
                        Ok(u) => u,
                        // Some servers reject CHARSET; retry without.
                        Err(_) => uid_search(c, &query.replacen("CHARSET UTF-8 ", "", 1)).await?,
                    };
                    let take: Vec<u32> = uids.into_iter().rev().take(100).collect();
                    if !take.is_empty() {
                        out.push((f.clone(), sel.uid_validity.unwrap_or(0), uid_fetch(c, &im::uid_set(&take), &summary_query()).await?));
                    }
                }
                Ok(out)
            })?;
        let mut out = Vec::new();
        for (f, uv, items) in found {
            out.extend(self.summaries_from(mb, &f, uv, items, false));
        }
        out.sort_by(|a, b| b.received_at.cmp(&a.received_at));
        out.truncate(100);
        Ok(out)
    }

    // ---- single message ----

    pub(crate) async fn imap_raw(&self, mb: &str, id: &str) -> Result<(Vec<u8>, Vec<String>, Option<DateTime<chrono::FixedOffset>>)> {
        let (uv, uid, folder) = im::parse_message_id(id).ok_or_else(|| Error::invalid("Ungültige Nachrichten-ID"))?;
        let folder = folder.to_string();
        with_imap!(self, mb, |c| {
            let sel = c.select(&folder).await?;
            if sel.uid_validity.unwrap_or(0) != uv {
                return Err(Error::NotFound("Nachricht (Ordner wurde neu aufgebaut)".into()));
            }
            let f = uid_fetch(c, &uid.to_string(), "(UID FLAGS INTERNALDATE BODY.PEEK[])")
                .await?
                .into_iter()
                .find(|f| f.uid == uid)
                .ok_or_else(|| Error::NotFound("Nachricht".into()))?;
            Ok((f.body.unwrap_or_default(), f.flags, f.internal_date))
        })
    }

    pub(crate) async fn imap_get_message(&self, mb: &str, id: &str) -> Result<MessageDetail> {
        let (raw, flags, date) = match self.imap_raw(mb, id).await {
            Ok(x) => x,
            Err(e) if e.is_network() => {
                let mut d = self.store.get_detail(mb, id)?.ok_or(e)?;
                if let Some(s) = self.store.get_summary(mb, id)? {
                    d.summary = s;
                }
                return Ok(d);
            }
            Err(e) => return Err(e),
        };
        let (_, _, folder) = im::parse_message_id(id).expect("checked");
        let summary = match self.store.get_summary(mb, id)? {
            Some(s) => s,
            None => mime::summary(mb, folder, id.to_string(), &raw, None, &flags, date),
        };
        let d = mime::detail(&raw, summary)?;
        self.store.put_detail(&d)?;
        if self.store.message_exists(mb, id)? {
            self.store.upsert_messages(std::slice::from_ref(&d.summary))?;
        }
        Ok(d)
    }

    pub(crate) async fn imap_message_source(&self, mb: &str, id: &str) -> Result<String> {
        let (raw, _, _) = self.imap_raw(mb, id).await?;
        Ok(String::from_utf8_lossy(&raw).into_owned())
    }

    pub(crate) async fn imap_download_attachment(&self, mb: &str, id: &str, att: &str, action: &str) -> Result<Option<String>> {
        let (raw, _, _) = self.imap_raw(mb, id).await?;
        let idx: usize = att.parse().map_err(|_| Error::invalid("Ungültige Anhang-ID"))?;
        let (name, _, bytes) = mime::attachment(&raw, idx)?;
        let name = super::mail::sanitize_filename(&name);
        let path = if action == "save" {
            match self.host.save_dialog(&name).await {
                Some(p) => p,
                None => return Ok(None),
            }
        } else {
            let dir = std::env::temp_dir().join("outlook-clone").join(uuid::Uuid::new_v4().to_string());
            std::fs::create_dir_all(&dir)?;
            dir.join(&name)
        };
        std::fs::write(&path, bytes)?;
        if action != "save" {
            self.host.open_path(&path)?;
        }
        Ok(Some(path.to_string_lossy().into_owned()))
    }

    // ---- flags & categories ----

    async fn imap_store(&self, mb: &str, ids: &[String], ops: &[String]) -> Result<()> {
        let groups = by_folder(ids)?;
        with_imap!(self, mb, |c| {
            for (folder, (_, uids)) in &groups {
                c.select(folder).await?;
                for op in ops {
                    store_flags(c, &im::uid_set(uids), op).await?;
                }
            }
            Ok(())
        })
    }

    fn emit_changed_folders(&self, mb: &str, folders: HashSet<String>) {
        for f in folders {
            self.emit_mail_changed(mb, Some(&f));
        }
    }

    pub(crate) async fn imap_set_read(&self, mb: &str, ids: &[String], read: bool) -> Result<()> {
        let op = if read { "+FLAGS.SILENT (\\Seen)" } else { "-FLAGS.SILENT (\\Seen)" };
        self.imap_store(mb, ids, &[op.to_string()]).await?;
        let f = self.update_cached(mb, ids, |m| m.is_read = read)?;
        self.emit_changed_folders(mb, f);
        Ok(())
    }

    pub(crate) async fn imap_set_flag(&self, mb: &str, ids: &[String], flag: &str) -> Result<()> {
        let ops: &[&str] = match flag {
            "flagged" => &["+FLAGS.SILENT (\\Flagged)", "-FLAGS.SILENT ($Completed)"],
            "complete" => &["-FLAGS.SILENT (\\Flagged)", "+FLAGS.SILENT ($Completed)"],
            "notFlagged" => &["-FLAGS.SILENT (\\Flagged $Completed)"],
            _ => return Err(Error::invalid("Ungültige Kennzeichnung")),
        };
        let ops: Vec<String> = ops.iter().map(|s| s.to_string()).collect();
        self.imap_store(mb, ids, &ops).await?;
        let f = self.update_cached(mb, ids, |m| m.flag = flag.to_string())?;
        self.emit_changed_folders(mb, f);
        Ok(())
    }

    /// Categories become IMAP keywords. Servers that do not allow new
    /// keywords (no `\*` in PERMANENTFLAGS) keep them only in the local cache.
    pub(crate) async fn imap_set_categories(&self, mb: &str, ids: &[String], categories: &[String]) -> Result<()> {
        let groups = by_folder(ids)?;
        let wanted: HashSet<String> = categories.iter().map(|c| im::category_keyword(c)).collect();
        with_imap!(self, mb, |c| {
            for (folder, (_, uids)) in &groups {
                let sel = c.select(folder).await?;
                if !may_create_keywords(&sel) {
                    continue;
                }
                let current = uid_fetch(c, &im::uid_set(uids), "(UID FLAGS)").await?;
                let mut remove: HashSet<String> = HashSet::new();
                for f in &current {
                    remove.extend(f.flags.iter().filter(|x| x.starts_with("$cat_") && !wanted.contains(*x)).cloned());
                }
                if !remove.is_empty() {
                    let list: Vec<String> = remove.into_iter().collect();
                    store_flags(c, &im::uid_set(uids), &format!("-FLAGS.SILENT ({})", list.join(" "))).await?;
                }
                if !wanted.is_empty() {
                    let list: Vec<&String> = wanted.iter().collect();
                    let list: Vec<&str> = list.into_iter().map(String::as_str).collect();
                    store_flags(c, &im::uid_set(uids), &format!("+FLAGS.SILENT ({})", list.join(" "))).await?;
                }
            }
            Ok(())
        })?;
        self.imap_remember_categories(mb, categories)?;
        let f = self.update_cached(mb, ids, |m| m.categories = categories.to_vec())?;
        self.emit_changed_folders(mb, f);
        Ok(())
    }

    // ---- move / delete / archive ----

    pub(crate) async fn imap_move(&self, mb: &str, ids: &[String], dest: &str, copy: bool) -> Result<Vec<(String, String)>> {
        let groups = by_folder(ids)?;
        with_imap!(self, mb, |c| {
            for (folder, (_, uids)) in &groups {
                if folder == dest && !copy {
                    continue;
                }
                c.select(folder).await?;
                transfer(c, uids, dest, copy).await?;
            }
            Ok(())
        })?;
        let mut folders: HashSet<String> = HashSet::from([dest.to_string()]);
        if !copy {
            for id in ids {
                if let Some(old) = self.store.get_summary(mb, id)? {
                    if old.folder_id == dest {
                        continue;
                    }
                    self.store
                        .adjust_folder_counts(mb, &old.folder_id, if old.is_read { 0 } else { -1 }, -1)?;
                    folders.insert(old.folder_id);
                }
                self.store.delete_message(mb, id)?;
            }
        }
        // Pick up the new UIDs in the destination.
        let _ = self.imap_sync_folder(mb, dest, true).await;
        self.emit_changed_folders(mb, folders);
        Ok(vec![])
    }

    pub(crate) async fn imap_delete(&self, mb: &str, ids: &[String], permanent: bool) -> Result<()> {
        let trash = self.store.folder_by_well_known(mb, "deleteditems")?.map(|f| f.id);
        let groups = by_folder(ids)?;
        let mut to_trash = Vec::new();
        let mut purge: HashMap<String, Vec<u32>> = HashMap::new();
        for (folder, (v, uids)) in &groups {
            if permanent || trash.is_none() || trash.as_deref() == Some(folder.as_str()) {
                purge.insert(folder.clone(), uids.clone());
            } else {
                to_trash.extend(uids.iter().map(|u| im::message_id(*v, *u, folder)));
            }
        }
        if let (Some(t), false) = (&trash, to_trash.is_empty()) {
            self.imap_move(mb, &to_trash, t, false).await?;
        }
        if !purge.is_empty() {
            with_imap!(self, mb, |c| {
                for (folder, uids) in &purge {
                    c.select(folder).await?;
                    expunge_uids(c, uids).await?;
                }
                Ok(())
            })?;
            let mut folders = HashSet::new();
            for id in ids {
                if let Some(old) = self.store.get_summary(mb, id)? {
                    if purge.contains_key(&old.folder_id) {
                        self.store
                            .adjust_folder_counts(mb, &old.folder_id, if old.is_read { 0 } else { -1 }, -1)?;
                        folders.insert(old.folder_id.clone());
                        self.store.delete_message(mb, id)?;
                    }
                }
            }
            self.emit_changed_folders(mb, folders);
        }
        Ok(())
    }

    /// Folder with a role, created at the top level when missing.
    pub(crate) async fn imap_ensure_folder(&self, mb: &str, well_known: &str, name: &str) -> Result<Folder> {
        if let Some(f) = self.store.folder_by_well_known(mb, well_known)? {
            return Ok(f);
        }
        let full = self.imap_child_name(mb, None, name)?;
        let r = with_imap!(self, mb, |c| {
                timed(c.s().create(&full)).await?;
                let _ = timed(c.s().subscribe(&full)).await;
                Ok(())
            });
        if let Err(e) = r {
            // It may already exist (e.g. created by another client).
            tracing::debug!("CREATE {full}: {e}");
        }
        self.imap_sync_folders(mb).await?;
        let mut f = self
            .store
            .folder_by_well_known(mb, well_known)?
            .or(self.store.get_folder(mb, &full)?)
            .ok_or_else(|| Error::NotFound(format!("Ordner {name}")))?;
        if f.well_known.is_none() {
            f.well_known = Some(well_known.to_string());
            self.store.upsert_folder(&f)?;
        }
        self.emit_mail_changed(mb, None);
        Ok(f)
    }

    pub(crate) async fn imap_archive(&self, mb: &str, ids: &[String]) -> Result<()> {
        let f = self.imap_ensure_folder(mb, "archive", ARCHIVE_FOLDER).await?;
        self.imap_move(mb, ids, &f.id, false).await?;
        Ok(())
    }

    // ---- Wiedervorlage ----

    /// Message-ID header of a message (cached summary or from the server).
    async fn imap_message_ids(&self, mb: &str, ids: &[String]) -> Result<Vec<String>> {
        let mut out = Vec::new();
        let mut missing = Vec::new();
        for id in ids {
            match self.store.get_summary(mb, id)?.map(|s| s.conversation_id).filter(|c| !c.is_empty()) {
                Some(m) => out.push(m),
                None => missing.push(id.clone()),
            }
        }
        if !missing.is_empty() {
            let groups = by_folder(&missing)?;
            let fetched = with_imap!(self, mb, |c| {
                    let mut v = Vec::new();
                    for (folder, (_, uids)) in &groups {
                        c.select(folder).await?;
                        v.extend(uid_fetch(c, &im::uid_set(uids), "(UID BODY.PEEK[HEADER.FIELDS (MESSAGE-ID)])").await?);
                    }
                    Ok(v)
                })?;
            for f in fetched {
                let raw = f.header.or(f.body).unwrap_or_default();
                if let Some(m) = mime::parse(&raw).and_then(|p| p.message_id().map(str::to_string)) {
                    out.push(m);
                }
            }
        }
        Ok(out)
    }

    pub(crate) async fn imap_snooze(&self, mb: &str, ids: &[String], until: &str) -> Result<()> {
        let until: DateTime<Utc> = DateTime::parse_from_rfc3339(until)
            .map_err(|_| Error::invalid("Ungültiger Zeitpunkt für die Wiedervorlage"))?
            .with_timezone(&Utc);
        if until <= Utc::now() {
            return Err(Error::invalid("Der Zeitpunkt der Wiedervorlage liegt in der Vergangenheit."));
        }
        let due = until.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let folder = self.imap_ensure_folder(mb, "snoozed", SNOOZE_FOLDER).await?;
        for mid in self.imap_message_ids(mb, ids).await? {
            self.store.kv_set(&snooze_key(mb, &mid), &due)?;
        }
        let (already, to_move): (Vec<String>, Vec<String>) = ids.iter().cloned().partition(|id| {
            im::parse_message_id(id).is_some_and(|(_, _, f)| f == folder.id)
        });
        self.update_cached(mb, &already, |m| m.due_at = Some(due.clone()))?;
        if !to_move.is_empty() {
            self.imap_move(mb, &to_move, &folder.id, false).await?;
        }
        self.emit_mail_changed(mb, Some(&folder.id));
        Ok(())
    }

    pub(crate) async fn imap_unsnooze(&self, mb: &str, ids: &[String]) -> Result<()> {
        for mid in self.imap_message_ids(mb, ids).await? {
            self.store.kv_set(&snooze_key(mb, &mid), "")?;
        }
        self.imap_store(mb, ids, &["-FLAGS.SILENT (\\Seen)".to_string()]).await?;
        self.imap_move(mb, ids, "INBOX", false).await?;
        Ok(())
    }

    pub(crate) async fn imap_process_due_snoozes(&self, mb: &str) -> Result<()> {
        let Some(folder) = self.store.folder_by_well_known(mb, "snoozed")? else {
            return Ok(());
        };
        self.imap_sync_folder(mb, &folder.id, true).await?;
        let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let due: Vec<MessageSummary> = self
            .store
            .list_messages(&ListQuery {
                mailbox_id: mb.into(),
                folder_id: folder.id.clone(),
                limit: 10_000,
                ..Default::default()
            })?
            .items
            .into_iter()
            .filter(|m| m.due_at.as_deref().is_some_and(|d| !d.is_empty() && d <= now.as_str()))
            .collect();
        if due.is_empty() {
            return Ok(());
        }
        let ids: Vec<String> = due.iter().map(|m| m.id.clone()).collect();
        self.imap_unsnooze(mb, &ids).await?;
        let first = &due[0];
        let from = first
            .from
            .as_ref()
            .map(|a| if a.name.is_empty() { a.address.clone() } else { a.name.clone() })
            .unwrap_or_default();
        self.emit(
            "new-mail",
            json!({"mailboxId": mb, "count": due.len(), "from": from, "subject": first.subject}),
        );
        let title = if due.len() == 1 {
            "Wiedervorlage".to_string()
        } else {
            format!("{} Wiedervorlagen", due.len())
        };
        self.host.notify(&title, &first.subject);
        Ok(())
    }

    // ---- local category list ----

    pub(crate) fn imap_categories(&self, account: &str) -> Result<Vec<Category>> {
        Ok(self.store.get_account_json(account, "categories")?.unwrap_or_default())
    }

    fn imap_save_categories(&self, account: &str, cats: &[Category]) -> Result<()> {
        self.store.put_account_json(account, "categories", &cats)?;
        self.emit("categories-changed", json!({"accountId": account}));
        Ok(())
    }

    /// Adds category names used on messages to the account's list.
    fn imap_remember_categories(&self, account: &str, names: &[String]) -> Result<()> {
        let mut cats = self.imap_categories(account)?;
        let mut changed = false;
        for n in names {
            if !cats.iter().any(|c| c.display_name.eq_ignore_ascii_case(n)) {
                cats.push(Category {
                    id: uuid::Uuid::new_v4().to_string(),
                    display_name: n.clone(),
                    color: "none".into(),
                });
                changed = true;
            }
        }
        if changed {
            self.imap_save_categories(account, &cats)?;
        }
        Ok(())
    }

    pub(crate) fn imap_create_category(&self, account: &str, name: &str, color: &str) -> Result<Category> {
        let name = name.trim();
        if name.is_empty() {
            return Err(Error::invalid("Name der Kategorie fehlt."));
        }
        valid_color(color)?;
        let mut cats = self.imap_categories(account)?;
        if cats.iter().any(|c| c.display_name.eq_ignore_ascii_case(name)) {
            return Err(Error::invalid(format!("Die Kategorie \"{name}\" existiert bereits.")));
        }
        let c = Category {
            id: uuid::Uuid::new_v4().to_string(),
            display_name: name.to_string(),
            color: color.to_string(),
        };
        cats.push(c.clone());
        self.imap_save_categories(account, &cats)?;
        Ok(c)
    }

    pub(crate) fn imap_update_category(&self, account: &str, id: &str, color: &str) -> Result<Category> {
        valid_color(color)?;
        let mut cats = self.imap_categories(account)?;
        let c = cats
            .iter_mut()
            .find(|c| c.id == id)
            .ok_or_else(|| Error::NotFound("Kategorie".into()))?;
        c.color = color.to_string();
        let out = c.clone();
        self.imap_save_categories(account, &cats)?;
        Ok(out)
    }

    pub(crate) fn imap_delete_category(&self, account: &str, id: &str) -> Result<()> {
        let mut cats = self.imap_categories(account)?;
        cats.retain(|c| c.id != id);
        self.imap_save_categories(account, &cats)
    }
}

fn valid_color(color: &str) -> Result<()> {
    let ok = color == "none"
        || color
            .strip_prefix("preset")
            .and_then(|n| n.parse::<u8>().ok())
            .is_some_and(|n| n <= 24);
    if ok {
        Ok(())
    } else {
        Err(Error::invalid(format!("Ungültige Farbe: {color}")))
    }
}
