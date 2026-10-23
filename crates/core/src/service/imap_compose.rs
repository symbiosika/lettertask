//! Drafts and sending for IMAP accounts: drafts are MIME messages in the
//! Drafts folder (replaced on every save), sending goes through SMTP and a
//! copy is stored in Sent.

use base64::Engine;
use serde_json::json;

use super::compose::{cid_to_data, data_to_cid, prepend_to_body};
use super::imap_mail::{expunge_uids, uid_search};
use super::signatures::signature_block;
use super::imap_account::with_imap;
use super::Service;
use crate::error::{Error, Result};
use crate::imap::conn::timed;
use crate::imap::mime::{self, Outgoing};
use crate::imap::{self as im};
use crate::model::*;

const REPLY_PREFIXES: &[&str] = &["aw:", "re:", "antw:", "sv:"];
const FORWARD_PREFIXES: &[&str] = &["wg:", "fw:", "fwd:", "wtrlt:"];

/// "AW: x" / "WG: x" without stacking prefixes.
fn prefixed(subject: &str, prefix: &str, known: &[&str]) -> String {
    let mut rest = subject.trim();
    loop {
        let lower = rest.to_lowercase();
        match known.iter().find(|p| lower.starts_with(*p)) {
            Some(p) => rest = rest[p.len()..].trim_start(),
            None => break,
        }
    }
    format!("{prefix} {rest}")
}

fn display(a: &Address) -> String {
    if a.name.is_empty() {
        a.address.clone()
    } else {
        format!("{} <{}>", a.name, a.address)
    }
}

fn esc(s: &str) -> String {
    html_escape::encode_text(s).into_owned()
}

/// Content between `<body>` and `</body>` (or the whole HTML).
fn inner_body(html: &str) -> &str {
    let lower = html.to_ascii_lowercase();
    let start = lower
        .find("<body")
        .and_then(|p| lower[p..].find('>').map(|e| p + e + 1))
        .unwrap_or(0);
    let end = lower.rfind("</body>").filter(|e| *e >= start).unwrap_or(html.len());
    &html[start..end]
}

fn same(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

/// A draft as stored on the server, parsed.
struct StoredDraft {
    message_id: String,
    in_reply_to: Option<String>,
    references: Option<String>,
    parts: Vec<mime::Part>,
}

impl Service {
    fn imap_from(&self, mb: &str) -> Result<Address> {
        let s = self.imap_settings(mb)?;
        Ok(Address {
            name: s.display_name.clone(),
            address: s.email.clone(),
        })
    }

    /// Appends `raw` to `folder` and returns the new message id.
    async fn imap_append(&self, mb: &str, folder: &str, flags: &str, raw: &[u8], message_id: &str) -> Result<String> {
        let header = format!("HEADER Message-ID {}", im::quote(message_id));
        with_imap!(self, mb, |c| {
            c.selected = None;
            timed(c.s().append(folder, Some(flags), None, raw)).await?;
            let sel = c.select(folder).await?;
            let uid = uid_search(c, &header)
                .await?
                .into_iter()
                .max()
                .ok_or_else(|| Error::Other("Gespeicherte Nachricht nicht gefunden".into()))?;
            Ok(im::message_id(sel.uid_validity.unwrap_or(0), uid, folder))
        })
    }

    /// Stores a complete message (from another mailbox) in `folder`.
    pub(crate) async fn imap_import(&self, mb: &str, folder: &str, raw: &[u8], seen: bool) -> Result<()> {
        let flags = if seen { "(\\Seen)" } else { "()" };
        with_imap!(self, mb, |c| {
            c.selected = None;
            timed(c.s().append(folder, Some(flags), None, raw)).await?;
            Ok::<(), Error>(())
        })?;
        Ok(())
    }

    async fn imap_remove(&self, mb: &str, id: &str) -> Result<()> {
        let Some((uv, uid, folder)) = im::parse_message_id(id) else {
            return Ok(());
        };
        let folder = folder.to_string();
        with_imap!(self, mb, |c| {
            c.select_checked(&folder, uv).await?;
            expunge_uids(c, &[uid]).await
        })?;
        self.store.delete_message(mb, id)?;
        Ok(())
    }

    async fn imap_stored_draft(&self, mb: &str, id: &str) -> Result<(Vec<u8>, StoredDraft)> {
        let (raw, _, _) = self.imap_raw(mb, id).await?;
        let parsed = mime::parse(&raw).ok_or_else(|| Error::Other("Entwurf nicht lesbar".into()))?;
        let header_ids = |name: &str| -> Option<String> {
            parsed.header_raw(name).map(|v| v.split_whitespace().collect::<Vec<_>>().join(" ")).filter(|v| !v.is_empty())
        };
        let d = StoredDraft {
            message_id: parsed
                .message_id()
                .map(|m| format!("<{m}>"))
                .unwrap_or_else(|| mime::new_message_id("localhost")),
            in_reply_to: header_ids("In-Reply-To"),
            references: header_ids("References"),
            parts: vec![],
        };
        drop(parsed);
        let parts = mime::all_attachments(&raw);
        Ok((raw, StoredDraft { parts, ..d }))
    }

    async fn imap_load_draft(&self, mb: &str, id: &str, mode: &str) -> Result<Draft> {
        let (raw, stored) = self.imap_stored_draft(mb, id).await?;
        let msg = mime::parse(&raw).ok_or_else(|| Error::Other("Entwurf nicht lesbar".into()))?;
        let mut html = mime::html_body(&msg);
        let mut attachments = Vec::new();
        let refs = mime::cid_refs(&html);
        // Longest cid first: "cid:img1" must not replace inside "cid:img10".
        let mut parts: Vec<&mime::Part> = stored.parts.iter().collect();
        parts.sort_by_key(|p| std::cmp::Reverse(p.4.as_ref().map_or(0, String::len)));
        for (idx, name, ct, bytes, cid) in parts {
            if let Some(cid) = cid {
                if refs.contains(cid) {
                    let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
                    html = cid_to_data(&html, cid, ct, &b64);
                    continue;
                }
            }
            attachments.push(Attachment {
                id: idx.to_string(),
                name: name.clone(),
                content_type: ct.clone(),
                size: bytes.len() as i64,
                is_inline: false,
                content_id: cid.clone(),
            });
        }
        attachments.sort_by_key(|a| a.id.parse::<usize>().unwrap_or(0));
        Ok(Draft {
            id: Some(id.to_string()),
            mailbox_id: mb.to_string(),
            mode: mode.to_string(),
            to: mime::addresses(msg.to()),
            cc: mime::addresses(msg.cc()),
            bcc: mime::addresses(msg.bcc()),
            subject: msg.subject().unwrap_or("").to_string(),
            body_html: html,
            importance: mime::importance(&msg),
            request_read_receipt: msg.header_raw("Disposition-Notification-To").is_some(),
            attachments,
        })
    }

    pub(crate) async fn imap_get_draft(&self, mb: &str, id: &str) -> Result<Draft> {
        self.imap_load_draft(mb, id, "new").await
    }

    pub(crate) async fn imap_create_draft(&self, mb: &str, mode: &str, source: Option<&str>) -> Result<Draft> {
        let sig = self.signature_for(mb, mode);
        let block = signature_block(sig.as_ref());
        if mode == "new" {
            return Ok(Draft {
                id: None,
                mailbox_id: mb.to_string(),
                mode: "new".into(),
                to: vec![],
                cc: vec![],
                bcc: vec![],
                subject: String::new(),
                body_html: block,
                importance: "normal".into(),
                request_read_receipt: false,
                attachments: vec![],
            });
        }
        if !matches!(mode, "reply" | "replyAll" | "forward") {
            return Err(Error::invalid("Unbekannter Modus"));
        }
        let source = source.ok_or_else(|| Error::invalid("Ursprungsnachricht fehlt"))?;
        let me = self.imap_from(mb)?;
        let (raw, _, _) = self.imap_raw(mb, source).await?;
        let msg = mime::parse(&raw).ok_or_else(|| Error::Other("Nachricht nicht lesbar".into()))?;
        let from = mime::addresses(msg.from());
        let to = mime::addresses(msg.to());
        let cc = mime::addresses(msg.cc());
        let reply_to = mime::addresses(msg.reply_to());
        let subject = msg.subject().unwrap_or("").to_string();

        // Replying to an own (sent) mail goes to its recipients, like Outlook.
        let own = !from.is_empty() && from.iter().all(|a| same(&a.address, &me.address));
        let sender = if own && !to.is_empty() {
            to.clone()
        } else if reply_to.is_empty() {
            from.clone()
        } else {
            reply_to.clone()
        };
        let mut sender_dedup: Vec<Address> = Vec::new();
        for a in sender {
            if !sender_dedup.iter().any(|x| same(&x.address, &a.address)) {
                sender_dedup.push(a);
            }
        }
        let sender = sender_dedup;
        let (out_to, out_cc, out_subject) = match mode {
            "reply" => (sender.clone(), vec![], prefixed(&subject, "AW:", REPLY_PREFIXES)),
            "replyAll" => {
                let mut t: Vec<Address> = sender.clone();
                for a in &to {
                    if !same(&a.address, &me.address) && !t.iter().any(|x| same(&x.address, &a.address)) {
                        t.push(a.clone());
                    }
                }
                let c: Vec<Address> = cc
                    .iter()
                    .filter(|a| !same(&a.address, &me.address) && !t.iter().any(|x| same(&x.address, &a.address)))
                    .cloned()
                    .collect();
                (t, c, prefixed(&subject, "AW:", REPLY_PREFIXES))
            }
            _ => (vec![], vec![], prefixed(&subject, "WG:", FORWARD_PREFIXES)),
        };

        let sent = msg
            .date()
            .and_then(|d| chrono::DateTime::from_timestamp(d.to_timestamp(), 0))
            .map(|d| d.with_timezone(&chrono::Local).format("%d.%m.%Y %H:%M").to_string())
            .unwrap_or_default();
        let list = |v: &[Address]| v.iter().map(display).collect::<Vec<_>>().join("; ");
        let mut header = format!(
            "<b>Von:</b> {}<br><b>Gesendet:</b> {}<br><b>An:</b> {}<br>",
            esc(&list(&from)),
            esc(&sent),
            esc(&list(&to))
        );
        if !cc.is_empty() {
            header.push_str(&format!("<b>Cc:</b> {}<br>", esc(&list(&cc))));
        }
        header.push_str(&format!("<b>Betreff:</b> {}", esc(&subject)));
        let original = mime::html_body(&msg);
        let quoted = format!(
            "<div><br></div><hr style=\"border:none;border-top:1px solid #e1e1e1\"><div style=\"font-size:11pt\">{header}</div><br>{}",
            inner_body(&original)
        );
        let body = prepend_to_body(&quoted, &block);

        let parts = mime::all_attachments(&raw);
        let body_refs = mime::cid_refs(&body);
        let inline = parts
            .iter()
            .filter_map(|(_, _, ct, bytes, cid)| {
                let cid = cid.clone()?;
                body_refs.contains(&cid).then(|| (cid, ct.clone(), bytes.clone()))
            })
            .collect();
        let attachments = if mode == "forward" {
            parts
                .iter()
                .filter(|(_, _, _, _, cid)| cid.as_ref().is_none_or(|c| !body_refs.contains(c)))
                .map(|(_, n, ct, b, _)| (n.clone(), ct.clone(), b.clone()))
                .collect()
        } else {
            vec![]
        };
        let orig_id = msg.message_id().map(|m| format!("<{m}>"));
        let references = {
            // RFC 5322 3.6.4: parent's References, else its In-Reply-To.
            let mut r: Vec<String> = msg
                .header_raw("References")
                .or_else(|| msg.header_raw("In-Reply-To"))
                .map(|v| {
                    v.split_whitespace()
                        .filter(|t| t.starts_with('<') && t.ends_with('>'))
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            r.dedup();
            if let Some(o) = orig_id.as_ref().filter(|o| !r.contains(o)) {
                r.push(o.clone());
            }
            (!r.is_empty()).then(|| r.join(" "))
        };
        let o = Outgoing {
            from: me.clone(),
            to: out_to,
            cc: out_cc,
            subject: out_subject,
            html: body,
            importance: "normal".into(),
            message_id: mime::new_message_id(&me.address),
            in_reply_to: if mode == "forward" { None } else { orig_id },
            references: if mode == "forward" { None } else { references },
            attachments,
            inline,
            ..Default::default()
        };
        let id = self.imap_store_draft(mb, &o).await?;
        self.imap_load_draft(mb, &id, mode).await
    }

    /// APPENDs a draft to the Drafts folder; returns its id.
    async fn imap_store_draft(&self, mb: &str, o: &Outgoing) -> Result<String> {
        let drafts = self.imap_ensure_folder(mb, "drafts", "Drafts").await?;
        let raw = mime::build(o, true)?.formatted();
        let id = self.imap_append(mb, &drafts.id, "(\\Draft \\Seen)", &raw, &o.message_id).await?;
        let _ = self.imap_sync_folder(mb, &drafts.id, true).await;
        Ok(id)
    }

    /// Outgoing message from the compose input, keeping headers and
    /// attachments of the previously stored draft.
    async fn imap_outgoing(&self, d: &DraftInput) -> Result<Outgoing> {
        let me = self.imap_from(&d.mailbox_id)?;
        for a in d.to.iter().chain(&d.cc).chain(&d.bcc) {
            if !a.address.contains('@') {
                return Err(Error::invalid(format!("Ungültige Adresse: {}", a.address)));
            }
        }
        let html = data_to_cid(&d.body_html);
        let mut o = Outgoing {
            from: me.clone(),
            to: d.to.clone(),
            cc: d.cc.clone(),
            bcc: d.bcc.clone(),
            subject: d.subject.clone(),
            importance: d.importance.clone(),
            read_receipt: d.request_read_receipt,
            message_id: mime::new_message_id(&me.address),
            ..Default::default()
        };
        let html_refs = mime::cid_refs(&html);
        if let Some(id) = &d.id {
            let (_, stored) = self.imap_stored_draft(&d.mailbox_id, id).await?;
            o.message_id = stored.message_id;
            o.in_reply_to = stored.in_reply_to;
            o.references = stored.references;
            for (idx, name, ct, bytes, cid) in stored.parts {
                match cid {
                    Some(cid) if html_refs.contains(&cid) => o.inline.push((cid, ct, bytes)),
                    _ if d.removed_attachment_ids.contains(&idx.to_string()) => {}
                    _ => o.attachments.push((name, ct, bytes)),
                }
            }
        }
        for a in &d.new_attachments {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(a.content_base64.trim())
                .map_err(|e| Error::invalid(format!("Anhang {}: {e}", a.name)))?;
            o.attachments.push((a.name.clone(), a.content_type.clone(), bytes));
        }
        o.html = html;
        Ok(o)
    }

    pub(crate) async fn imap_save_draft(&self, d: DraftInput) -> Result<Draft> {
        let mb = d.mailbox_id.clone();
        // One save at a time per account: two overlapping saves of the same
        // draft would otherwise both append a new version.
        let lock = self.folder_lock(&mb, "\u{0}draft-save");
        let _g = lock.lock().await;
        let o = self.imap_outgoing(&d).await?;
        // Store the new version before removing the old one, so a failed
        // APPEND never loses the draft. Both carry the same Message-ID; the
        // append looks up the newest UID.
        let id = self.imap_store_draft(&mb, &o).await?;
        if let Some(old) = d.id.as_deref().filter(|old| *old != id) {
            if let Err(e) = self.imap_remove(&mb, old).await {
                tracing::warn!("Alter Entwurf nicht entfernt: {e}");
            }
            let drafts = self.store.folder_by_well_known(&mb, "drafts")?;
            if let Some(f) = drafts {
                let _ = self.imap_sync_folder(&mb, &f.id, true).await;
            }
        }
        self.imap_load_draft(&mb, &id, &d.mode).await
    }

    /// Returns the thread id (root Message-ID) of the sent mail.
    pub(crate) async fn imap_send_draft(&self, d: DraftInput) -> Result<Option<String>> {
        if d.to.is_empty() && d.cc.is_empty() && d.bcc.is_empty() {
            return Err(Error::invalid("Bitte mindestens einen Empfänger angeben."));
        }
        let mb = d.mailbox_id.clone();
        let o = self.imap_outgoing(&d).await?;
        let message = mime::build(&o, false)?;
        let settings = self.imap_settings(&mb)?;
        let credential = self.imap_credential(&mb, true).await?;
        crate::imap::smtp::send(&settings.smtp, &credential, message.envelope(), &message.formatted()).await?;

        // Copy to Sent (with Bcc, like Outlook); failures here must not
        // report the already delivered mail as unsent. Gmail files mails
        // sent over its SMTP server itself; a copy would show up twice.
        let gmail = self.is_gmail(&mb);
        let copy = mime::build(&o, true).map(|m| m.formatted());
        if let (Ok(copy), Ok(sent)) = (copy, self.imap_ensure_folder(&mb, "sentitems", "Sent").await) {
            if !gmail {
                if let Err(e) = self.imap_append(&mb, &sent.id, "(\\Seen)", &copy, &o.message_id).await {
                    tracing::warn!("Kopie in Gesendet fehlgeschlagen: {e}");
                }
            }
            let _ = self.imap_sync_folder(&mb, &sent.id, true).await;
            self.emit_mail_changed(&mb, Some(&sent.id));
        }
        if let Some(id) = &d.id {
            let _ = self.imap_remove(&mb, id).await;
        }
        if let Some(f) = self.store.folder_by_well_known(&mb, "drafts")? {
            self.emit_mail_changed(&mb, Some(&f.id));
        }
        let all: Vec<Address> = d
            .to
            .iter()
            .chain(&d.cc)
            .chain(&d.bcc)
            .map(|a| Address {
                name: mime::header_text(&a.name),
                address: a.address.trim().to_string(),
            })
            .collect();
        // The mail is out; failures from here on must not report it as unsent.
        if let Err(e) = self.collect_addresses(&mb, &all) {
            tracing::warn!("Adressen konnten nicht gemerkt werden: {e}");
        }
        if let Some(at) = d.remind_at.as_deref().filter(|a| !a.is_empty()) {
            if let Err(e) = self.create_send_reminder(&d, at, Some(o.message_id.trim_matches(['<', '>']))).await {
                tracing::warn!("Erinnerung konnte nicht angelegt werden: {e}");
                self.host.notify("Erinnerung konnte nicht angelegt werden", &format!("{}: {e}", d.subject));
                self.emit("reminder-failed", json!({"mailboxId": mb, "subject": d.subject, "message": e.to_string()}));
            }
        }
        Ok(Some(o.message_id.trim_matches(['<', '>']).to_string()))
    }

    /// Tasks/reminders: a normal (non-draft) mail from yourself, unread in
    /// the inbox, or read in "Wiedervorlage" with its due time.
    pub(crate) async fn imap_place_mail(&self, mb: &str, m: super::tasks::NewMail) -> Result<MessageSummary> {
        let me = self.imap_from(mb)?;
        let folder = match m.due {
            Some(_) => self.imap_ensure_folder(mb, "snoozed", super::imap_mail::SNOOZE_FOLDER).await?,
            None => self
                .store
                .folder_by_well_known(mb, "inbox")?
                .ok_or_else(|| Error::NotFound("Posteingang".into()))?,
        };
        let o = Outgoing {
            from: me.clone(),
            to: m.to.clone(),
            reply_to: m.reply_to.clone(),
            subject: m.subject.clone(),
            html: m.body_html.clone(),
            importance: "normal".into(),
            message_id: mime::new_message_id(&me.address),
            ..Default::default()
        };
        let keywords_ok = self.imap_keywords_ok(mb, &folder.id).await?;
        let mut flags: Vec<String> = Vec::new();
        if let Some(due) = m.due {
            flags.extend(["\\Seen".to_string(), "\\Flagged".to_string()]);
            if keywords_ok {
                flags.push(im::due_keyword(due));
            } else {
                let key = super::imap_mail::snooze_key(mb, o.message_id.trim_matches(['<', '>']));
                self.store
                    .kv_set(&key, &due.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))?;
            }
        }
        if keywords_ok {
            flags.extend(m.categories.iter().map(|c| im::category_keyword(c)));
        }
        let raw = mime::build(&o, true)?.formatted();
        let id = self
            .imap_append(mb, &folder.id, &format!("({})", flags.join(" ")), &raw, &o.message_id)
            .await?;
        self.imap_sync_folder(mb, &folder.id, true).await?;
        if !keywords_ok && !m.categories.is_empty() {
            // Server without custom keywords: categories only in the local cache.
            self.update_cached(mb, std::slice::from_ref(&id), |s| {
                s.categories = m.categories.clone();
                s.recurrence = crate::model::recurrence_of(&s.categories);
            })?;
        }
        self.emit_mail_changed(mb, Some(&folder.id));
        self.store
            .get_summary(mb, &id)?
            .ok_or_else(|| Error::NotFound("Nachricht".into()))
    }

    pub(crate) async fn imap_discard_draft(&self, mb: &str, id: Option<&str>) -> Result<()> {
        if let Some(id) = id {
            self.imap_remove(mb, id).await?;
            if let Some(f) = self.store.folder_by_well_known(mb, "drafts")? {
                self.emit_mail_changed(mb, Some(&f.id));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subject_prefixes() {
        assert_eq!(prefixed("Angebot", "AW:", REPLY_PREFIXES), "AW: Angebot");
        assert_eq!(prefixed("RE: AW: Angebot", "AW:", REPLY_PREFIXES), "AW: Angebot");
        assert_eq!(prefixed("Fwd: Bericht", "WG:", FORWARD_PREFIXES), "WG: Bericht");
    }

    #[test]
    fn body_extraction() {
        assert_eq!(inner_body("<html><body class=x><p>a</p></body></html>"), "<p>a</p>");
        assert_eq!(inner_body("<p>b</p>"), "<p>b</p>");
    }
}
