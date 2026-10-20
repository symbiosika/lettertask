//! "Archivieren" and "Auf Wiedervorlage" (snooze).
//!
//! Snoozed mails live in a normal mail folder "Wiedervorlage"; the return time
//! is stored as follow-up due date on the message itself, so it survives
//! reinstalls and is visible in Outlook on other devices. The background sync
//! moves due mails back to the inbox (unread).

use chrono::{DateTime, Utc};
use serde_json::json;

use super::Service;
use crate::error::{Error, Result};
use crate::model::Folder;

pub(crate) const SNOOZE_FOLDER_NAME: &str = "Wiedervorlage";
const ARCHIVE_FOLDER_NAME: &str = "Archiv";

fn graph_dt(dt: DateTime<Utc>) -> serde_json::Value {
    json!({"dateTime": dt.format("%Y-%m-%dT%H:%M:%S").to_string(), "timeZone": "UTC"})
}

fn snooze_key(mailbox_id: &str) -> String {
    format!("snoozefolder|{mailbox_id}")
}

impl Service {
    /// Recognises the snooze folder: remembered id, or a top-level folder
    /// named "Wiedervorlage" (created by this app on another machine).
    pub(crate) fn is_snooze_folder(&self, mailbox_id: &str, f: &Folder) -> Result<bool> {
        match self.store.kv_get(&snooze_key(mailbox_id))? {
            Some(id) => Ok(id == f.id),
            None if f.parent_id.is_none() && f.display_name == SNOOZE_FOLDER_NAME => {
                self.store.kv_set(&snooze_key(mailbox_id), &f.id)?;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// Finds or creates a top-level folder with a fixed role.
    async fn ensure_folder(&self, mailbox_id: &str, well_known: &str, name: &str) -> Result<Folder> {
        if let Some(f) = self.store.folder_by_well_known(mailbox_id, well_known)? {
            return Ok(f);
        }
        let mut folder = match self.create_folder(mailbox_id, None, name).await {
            Ok(f) => f,
            Err(e @ Error::Graph { status: 409, .. }) => {
                // Exists on the server but not in our cache yet.
                self.sync_folders(mailbox_id).await?;
                self.store
                    .list_folders(mailbox_id)?
                    .into_iter()
                    .find(|f| f.parent_id.is_none() && f.display_name == name)
                    .ok_or(e)?
            }
            Err(e) => return Err(e),
        };
        if well_known == "snoozed" {
            self.store.kv_set(&snooze_key(mailbox_id), &folder.id)?;
        }
        folder.well_known = Some(well_known.to_string());
        self.store.upsert_folder(&folder)?;
        self.emit_mail_changed(mailbox_id, None);
        Ok(folder)
    }

    pub(crate) async fn archive_messages(&self, mailbox_id: &str, ids: &[String]) -> Result<()> {
        let archive = self.ensure_folder(mailbox_id, "archive", ARCHIVE_FOLDER_NAME).await?;
        self.move_messages(mailbox_id, ids, &archive.id, false).await?;
        Ok(())
    }

    pub(crate) async fn snooze_messages(&self, mailbox_id: &str, ids: &[String], until: &str) -> Result<()> {
        let until: DateTime<Utc> = DateTime::parse_from_rfc3339(until)
            .map_err(|_| Error::invalid("Ungültiger Zeitpunkt für die Wiedervorlage"))?
            .with_timezone(&Utc);
        if until <= Utc::now() {
            return Err(Error::invalid("Der Zeitpunkt der Wiedervorlage liegt in der Vergangenheit."));
        }
        let folder = self.ensure_folder(mailbox_id, "snoozed", SNOOZE_FOLDER_NAME).await?;
        let (mb, c) = self.mailbox(mailbox_id)?;
        let flag = json!({
            "flagStatus": "flagged",
            "startDateTime": graph_dt(Utc::now()),
            "dueDateTime": graph_dt(until),
        });
        self.patch_many(&mb, &c, ids, json!({ "flag": flag })).await?;
        let due = until.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        self.update_cached(mailbox_id, ids, |m| {
            m.flag = "flagged".into();
            m.due_at = Some(due.clone());
        })?;
        // Messages already in the snooze folder only get the new time.
        let to_move: Vec<String> = ids
            .iter()
            .filter(|id| {
                self.store
                    .get_summary(mailbox_id, id)
                    .ok()
                    .flatten()
                    .is_none_or(|m| m.folder_id != folder.id)
            })
            .cloned()
            .collect();
        if !to_move.is_empty() {
            self.move_messages(mailbox_id, &to_move, &folder.id, false).await?;
        }
        self.emit_mail_changed(mailbox_id, Some(&folder.id));
        Ok(())
    }

    pub(crate) async fn unsnooze_messages(&self, mailbox_id: &str, ids: &[String]) -> Result<()> {
        let inbox = self
            .store
            .folder_by_well_known(mailbox_id, "inbox")?
            .ok_or_else(|| Error::NotFound("Posteingang".into()))?;
        let mapping = self.move_messages(mailbox_id, ids, &inbox.id, false).await?;
        let new_ids: Vec<String> = mapping.into_iter().map(|(_, n)| n).collect();
        let (mb, c) = self.mailbox(mailbox_id)?;
        self.patch_many(
            &mb,
            &c,
            &new_ids,
            json!({"isRead": false, "flag": {"flagStatus": "notFlagged"}}),
        )
        .await?;
        self.update_cached(mailbox_id, &new_ids, |m| {
            m.is_read = false;
            m.flag = "notFlagged".into();
            m.due_at = None;
        })?;
        self.emit_mail_changed(mailbox_id, Some(&inbox.id));
        Ok(())
    }

    /// Returns due snoozed mails to the inbox. Called by the background sync.
    pub(crate) async fn process_due_snoozes(&self, mailbox_id: &str) -> Result<()> {
        let Some(folder) = self.store.folder_by_well_known(mailbox_id, "snoozed")? else {
            return Ok(());
        };
        self.sync_folder(mailbox_id, &folder.id, true).await?;
        let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let due: Vec<_> = self
            .store
            .list_messages(&crate::model::ListQuery {
                mailbox_id: mailbox_id.into(),
                folder_id: folder.id.clone(),
                limit: 10_000,
                ..Default::default()
            })?
            .items
            .into_iter()
            .filter(|m| m.due_at.as_deref().is_some_and(|d| d <= now.as_str()))
            .collect();
        if due.is_empty() {
            return Ok(());
        }
        let ids: Vec<String> = due.iter().map(|m| m.id.clone()).collect();
        self.unsnooze_messages(mailbox_id, &ids).await?;
        let first = &due[0];
        let from = first
            .from
            .as_ref()
            .map(|a| if a.name.is_empty() { a.address.clone() } else { a.name.clone() })
            .unwrap_or_default();
        self.emit(
            "new-mail",
            json!({"mailboxId": mailbox_id, "count": due.len(), "from": from, "subject": first.subject}),
        );
        let title = if due.len() == 1 {
            "Wiedervorlage".to_string()
        } else {
            format!("{} Wiedervorlagen", due.len())
        };
        self.host.notify(&title, &first.subject);
        Ok(())
    }
}
