//! Moving / copying mails between mailboxes (e.g. Microsoft 365 → Gmail).
//! Servers cannot do that themselves: the complete message is read from the
//! source and recreated in the target folder; a move then deletes the source.

use serde_json::{json, Value};

use super::Service;
use crate::convert;
use crate::error::{Error, Result};
use crate::graph::seg;
use crate::imap::mime;

impl Service {
    /// Returns the number of transferred mails.
    pub(crate) async fn transfer_messages(
        &self,
        mailbox_id: &str,
        ids: &[String],
        dest_mailbox_id: &str,
        dest_folder_id: &str,
        copy: bool,
    ) -> Result<usize> {
        if mailbox_id == dest_mailbox_id {
            self.move_messages(mailbox_id, ids, dest_folder_id, copy).await?;
            return Ok(ids.len());
        }
        let dest_folder = self
            .store
            .list_folders(dest_mailbox_id)?
            .into_iter()
            .find(|f| f.id == dest_folder_id)
            .ok_or_else(|| Error::NotFound("Zielordner".into()))?;
        let mut done = Vec::new();
        for id in ids {
            let seen = self.store.get_summary(mailbox_id, id)?.is_none_or(|m| m.is_read);
            let raw = self.raw_message(mailbox_id, id).await?;
            if crate::imap::is_imap_id(dest_mailbox_id) {
                self.imap_import(dest_mailbox_id, &dest_folder.id, &raw, seen).await?;
            } else {
                self.graph_import(dest_mailbox_id, &dest_folder.id, &raw, seen).await?;
            }
            done.push(id.clone());
        }
        // Only what arrived in the target leaves the source (into its trash).
        if !copy && !done.is_empty() {
            self.delete_messages(mailbox_id, &done, false).await?;
        }
        if crate::imap::is_imap_id(dest_mailbox_id) {
            self.imap_sync_folder(dest_mailbox_id, &dest_folder.id, true).await?;
        } else {
            self.sync_folder(dest_mailbox_id, &dest_folder.id, true).await?;
        }
        self.emit_mail_changed(dest_mailbox_id, Some(&dest_folder.id));
        self.emit_mail_changed(mailbox_id, None);
        Ok(done.len())
    }

    /// The complete message (RFC 822) from either kind of mailbox.
    async fn raw_message(&self, mailbox_id: &str, id: &str) -> Result<Vec<u8>> {
        if crate::imap::is_imap_id(mailbox_id) {
            return Ok(self.imap_raw(mailbox_id, id).await?.0);
        }
        let (mb, c) = self.mailbox(mailbox_id)?;
        c.get_bytes(&format!("{}/messages/{}/$value", mb.graph_prefix(), seg(id))).await
    }

    /// Recreates a message in a Microsoft 365 folder as a received (non-draft)
    /// mail with its original sender, recipients, dates and attachments.
    async fn graph_import(&self, mailbox_id: &str, folder_id: &str, raw: &[u8], seen: bool) -> Result<()> {
        let msg = mime::parse(raw).ok_or_else(|| Error::Other("Nachricht konnte nicht gelesen werden".into()))?;
        let (mb, c) = self.mailbox(mailbox_id)?;
        let prefix = mb.graph_prefix();
        let mut props = vec![json!({"id": "Integer 0x0E07", "value": if seen { "1" } else { "0" }})];
        if let Some(date) = msg.date() {
            let iso = date.to_rfc3339();
            // PidTagMessageDeliveryTime / PidTagClientSubmitTime: list sorts by the original date.
            props.push(json!({"id": "SystemTime 0x0E06", "value": iso}));
            props.push(json!({"id": "SystemTime 0x0039", "value": iso}));
        }
        let list = |a: Option<&mail_parser::Address<'_>>| convert::recipients_json(&mime::addresses(a));
        let mut body = json!({
            "subject": msg.subject().unwrap_or(""),
            "body": {"contentType": "html", "content": mime::html_body(&msg)},
            "toRecipients": list(msg.to()),
            "ccRecipients": list(msg.cc()),
            "singleValueExtendedProperties": props,
        });
        if let Some(from) = mime::addresses(msg.from()).into_iter().next() {
            body["from"] = json!({"emailAddress": {"name": from.name, "address": from.address}});
            body["sender"] = body["from"].clone();
        }
        let reply_to = mime::addresses(msg.reply_to());
        if !reply_to.is_empty() {
            body["replyTo"] = convert::recipients_json(&reply_to);
        }
        if let Some(mid) = msg.message_id() {
            body["internetMessageId"] = Value::String(format!("<{mid}>"));
        }
        let created = c
            .post(&format!("{prefix}/mailFolders/{}/messages", seg(folder_id)), &body)
            .await?;
        let new_id = created["id"].as_str().unwrap_or_default().to_string();
        for (_, name, content_type, bytes, cid) in mime::all_attachments(raw) {
            Self::upload_attachment(&c, &prefix, &new_id, &name, &content_type, &bytes, cid.as_deref()).await?;
        }
        Ok(())
    }
}
