use std::sync::Arc;

use base64::Engine;
use serde_json::{json, Value};

use super::signatures::signature_block;
use super::Service;
use crate::convert::{self, MESSAGE_DETAIL_SELECT};
use crate::error::{Error, Result};
use crate::graph::{seg, GraphClient};
use crate::model::*;

/// Attachments above this size must go through an upload session.
const SIMPLE_ATTACHMENT_LIMIT: usize = 3 * 1024 * 1024;
/// Upload chunk size; must be a multiple of 320 KiB.
const CHUNK: usize = 320 * 1024 * 10;

/// Inserts `fragment` right after the opening `<body>` tag (or at the start).
pub(crate) fn prepend_to_body(html: &str, fragment: &str) -> String {
    let lower = html.to_ascii_lowercase();
    if let Some(pos) = lower.find("<body") {
        if let Some(end) = lower[pos..].find('>') {
            let at = pos + end + 1;
            return format!("{}{}{}", &html[..at], fragment, &html[at..]);
        }
    }
    format!("{fragment}{html}")
}

impl Service {
    pub(crate) async fn create_draft(&self, mailbox_id: &str, mode: &str, source: Option<&str>) -> Result<Draft> {
        if crate::imap::is_imap_id(mailbox_id) {
            return self.imap_create_draft(mailbox_id, mode, source).await;
        }
        let account_id = self.store.get_mailbox(mailbox_id)?.account_id;
        let sig = signature_block(self.signature_for(&account_id, mode).as_ref());
        if mode == "new" {
            return Ok(Draft {
                id: None,
                mailbox_id: mailbox_id.to_string(),
                mode: "new".into(),
                to: vec![],
                cc: vec![],
                bcc: vec![],
                subject: String::new(),
                body_html: sig,
                importance: "normal".into(),
                request_read_receipt: false,
                attachments: vec![],
            });
        }
        let action = match mode {
            "reply" => "createReply",
            "replyAll" => "createReplyAll",
            "forward" => "createForward",
            _ => return Err(Error::invalid("Unbekannter Modus")),
        };
        let source = source.ok_or_else(|| Error::invalid("Ursprungsnachricht fehlt"))?;
        let (mb, c) = self.mailbox(mailbox_id)?;
        let v = c
            .post(
                &format!("{}/messages/{}/{action}", mb.graph_prefix(), seg(source)),
                &json!({}),
            )
            .await?;
        let id = v["id"].as_str().ok_or_else(|| Error::Other("Entwurf ohne ID".into()))?;
        let mut draft = self.load_draft(&mb, &c, id).await?;
        draft.mode = mode.to_string();
        draft.body_html = prepend_to_body(&draft.body_html, &sig);
        if let Some(d) = self.store.folder_by_well_known(mailbox_id, "drafts")? {
            self.emit_mail_changed(mailbox_id, Some(&d.id));
        }
        Ok(draft)
    }

    async fn load_draft(&self, mb: &Mailbox, c: &Arc<GraphClient>, id: &str) -> Result<Draft> {
        let v = c
            .get(&format!(
                "{}/messages/{}?$select={MESSAGE_DETAIL_SELECT}",
                mb.graph_prefix(),
                seg(id)
            ))
            .await?;
        let mut body_html = convert::body_html(&v);
        let mut attachments = Vec::new();
        if v["hasAttachments"].as_bool().unwrap_or(false) || body_html.contains("cid:") {
            let list = c
                .get_all(
                    &format!(
                        "{}/messages/{}/attachments?$select=id,name,contentType,size,isInline",
                        mb.graph_prefix(),
                        seg(id)
                    ),
                    &[],
                    500,
                )
                .await?;
            for a in list.iter().map(convert::attachment) {
                if a.is_inline && body_html.contains("cid:") {
                    // Show embedded images in the editor; `cid_to_data` is reversed on save.
                    if let Ok(full) = c
                        .get(&format!(
                            "{}/messages/{}/attachments/{}",
                            mb.graph_prefix(),
                            seg(id),
                            seg(&a.id)
                        ))
                        .await
                    {
                        let cid = full["contentId"].as_str().unwrap_or("").trim_matches(['<', '>']).to_string();
                        let bytes = full["contentBytes"].as_str().unwrap_or("");
                        if !cid.is_empty() && body_html.contains(&format!("cid:{cid}")) {
                            body_html = cid_to_data(&body_html, &cid, &a.content_type, bytes);
                            continue;
                        }
                    }
                }
                attachments.push(a);
            }
        }
        Ok(Draft {
            id: Some(id.to_string()),
            mailbox_id: mb.id.clone(),
            mode: "new".into(),
            to: convert::addresses(&v, "toRecipients"),
            cc: convert::addresses(&v, "ccRecipients"),
            bcc: convert::addresses(&v, "bccRecipients"),
            subject: v["subject"].as_str().unwrap_or("").to_string(),
            body_html,
            importance: v["importance"].as_str().unwrap_or("normal").to_string(),
            request_read_receipt: v["isReadReceiptRequested"].as_bool().unwrap_or(false),
            attachments,
        })
    }

    pub(crate) async fn get_draft(&self, mailbox_id: &str, message_id: &str) -> Result<Draft> {
        if crate::imap::is_imap_id(mailbox_id) {
            return self.imap_get_draft(mailbox_id, message_id).await;
        }
        let (mb, c) = self.mailbox(mailbox_id)?;
        self.load_draft(&mb, &c, message_id).await
    }

    fn draft_json(d: &DraftInput) -> Value {
        json!({
            "subject": d.subject,
            "body": {"contentType": "html", "content": d.body_html},
            "toRecipients": convert::recipients_json(&d.to),
            "ccRecipients": convert::recipients_json(&d.cc),
            "bccRecipients": convert::recipients_json(&d.bcc),
            "importance": d.importance,
            "isReadReceiptRequested": d.request_read_receipt,
        })
    }

    pub(crate) async fn save_draft(&self, d: DraftInput) -> Result<Draft> {
        if crate::imap::is_imap_id(&d.mailbox_id) {
            return self.imap_save_draft(d).await;
        }
        let (mb, c) = self.mailbox(&d.mailbox_id)?;
        let id = self.write_draft(&mb, &c, &d).await?;
        let mut draft = self.load_draft(&mb, &c, &id).await?;
        draft.mode = d.mode.clone();
        if let Some(f) = self.store.folder_by_well_known(&mb.id, "drafts")? {
            let _ = self.sync_folder(&mb.id, &f.id, true).await;
        }
        Ok(draft)
    }

    /// Creates/updates the server draft incl. attachments; returns its id.
    async fn write_draft(&self, mb: &Mailbox, c: &Arc<GraphClient>, d: &DraftInput) -> Result<String> {
        for a in d.to.iter().chain(&d.cc).chain(&d.bcc) {
            if !a.address.contains('@') {
                return Err(Error::invalid(format!("Ungültige Adresse: {}", a.address)));
            }
        }
        let prefix = mb.graph_prefix();
        let mut d = d.clone();
        d.body_html = data_to_cid(&d.body_html);
        let d = &d;
        let body = Self::draft_json(d);
        let id = match &d.id {
            Some(id) => {
                c.patch(&format!("{prefix}/messages/{}", seg(id)), &body).await?;
                id.clone()
            }
            None => {
                let v = c.post(&format!("{prefix}/messages"), &body).await?;
                v["id"]
                    .as_str()
                    .ok_or_else(|| Error::Other("Entwurf ohne ID".into()))?
                    .to_string()
            }
        };
        for aid in &d.removed_attachment_ids {
            match c
                .delete(&format!("{prefix}/messages/{}/attachments/{}", seg(&id), seg(aid)))
                .await
            {
                Err(e) if !e.is_not_found() => return Err(e),
                _ => {}
            }
        }
        for a in &d.new_attachments {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(a.content_base64.trim())
                .map_err(|e| Error::invalid(format!("Anhang {}: {e}", a.name)))?;
            if bytes.len() <= SIMPLE_ATTACHMENT_LIMIT {
                c.post(
                    &format!("{prefix}/messages/{}/attachments", seg(&id)),
                    &json!({
                        "@odata.type": "#microsoft.graph.fileAttachment",
                        "name": a.name,
                        "contentType": a.content_type,
                        "contentBytes": a.content_base64.trim(),
                    }),
                )
                .await?;
            } else {
                let session = c
                    .post(
                        &format!(
                            "{prefix}/messages/{}/attachments/createUploadSession",
                            seg(&id)
                        ),
                        &json!({"AttachmentItem": {
                            "attachmentType": "file",
                            "name": a.name,
                            "size": bytes.len(),
                            "contentType": a.content_type,
                        }}),
                    )
                    .await?;
                let url = session["uploadUrl"]
                    .as_str()
                    .ok_or_else(|| Error::Other("Upload-Sitzung ohne URL".into()))?;
                let total = bytes.len();
                let mut start = 0;
                while start < total {
                    let end = (start + CHUNK).min(total);
                    c.put_upload_chunk(url, bytes[start..end].to_vec(), start, total)
                        .await?;
                    start = end;
                }
            }
        }
        Ok(id)
    }

    pub(crate) async fn send_draft(&self, d: DraftInput) -> Result<()> {
        if crate::imap::is_imap_id(&d.mailbox_id) {
            return self.imap_send_draft(d).await;
        }
        if d.to.is_empty() && d.cc.is_empty() && d.bcc.is_empty() {
            return Err(Error::invalid("Bitte mindestens einen Empfänger angeben."));
        }
        let (mb, c) = self.mailbox(&d.mailbox_id)?;
        let id = self.write_draft(&mb, &c, &d).await?;
        // Thread id of the mail, for "Nachfassen erledigt sich selbst".
        let thread = if d.remind_at.as_deref().is_some_and(|a| !a.is_empty()) {
            c.get(&format!("{}/messages/{}?$select=conversationId", mb.graph_prefix(), seg(&id)))
                .await
                .ok()
                .and_then(|v| v["conversationId"].as_str().map(str::to_string))
        } else {
            None
        };
        c.post_empty(&format!("{}/messages/{}/send", mb.graph_prefix(), seg(&id)))
            .await?;
        let recipients: Vec<Address> = d.to.iter().chain(&d.cc).chain(&d.bcc).cloned().collect();
        self.collect_addresses(&mb.account_id, &recipients)?;
        if let Some(at) = d.remind_at.as_deref().filter(|a| !a.is_empty()) {
            self.create_send_reminder(&d, at, thread.as_deref()).await?;
        }
        self.store.delete_message(&mb.id, &id)?;
        for wk in ["drafts", "sentitems", "inbox"] {
            if let Some(f) = self.store.folder_by_well_known(&mb.id, wk)? {
                let _ = self.sync_folder(&mb.id, &f.id, true).await;
                self.emit_mail_changed(&mb.id, Some(&f.id));
            }
        }
        Ok(())
    }

    pub(crate) async fn discard_draft(&self, mailbox_id: &str, draft_id: Option<&str>) -> Result<()> {
        if crate::imap::is_imap_id(mailbox_id) {
            return self.imap_discard_draft(mailbox_id, draft_id).await;
        }
        let Some(id) = draft_id else { return Ok(()) };
        let (mb, c) = self.mailbox(mailbox_id)?;
        match c
            .delete(&format!("{}/messages/{}", mb.graph_prefix(), seg(id)))
            .await
        {
            Err(e) if !e.is_not_found() => return Err(e),
            _ => {}
        }
        self.store.delete_message(mailbox_id, id)?;
        if let Some(f) = self.store.folder_by_well_known(mailbox_id, "drafts")? {
            self.emit_mail_changed(mailbox_id, Some(&f.id));
        }
        Ok(())
    }

    pub(crate) async fn suggest_recipients(&self, mailbox_id: &str, query: &str) -> Result<Vec<Address>> {
        let q = query.trim();
        if q.len() < 2 {
            return Ok(vec![]);
        }
        let mut out: Vec<Address> = Vec::new();
        let push = |a: Address, out: &mut Vec<Address>| {
            if !a.address.is_empty()
                && !out.iter().any(|x| x.address.eq_ignore_ascii_case(&a.address))
            {
                out.push(a);
            }
        };
        for a in self.contact_suggestions(q, 10)? {
            push(a, &mut out);
        }
        if let Ok((mb, c)) = self.mailbox(mailbox_id) {
            let _ = mb;
            let path = format!(
                "/me/people?$search={}&$top=10&$select=displayName,scoredEmailAddresses",
                urlencoding::encode(&format!("\"{}\"", q.replace('"', " ")))
            );
            if let Ok(v) = c.get(&path).await {
                for p in v["value"].as_array().cloned().unwrap_or_default() {
                    let name = p["displayName"].as_str().unwrap_or("").to_string();
                    if let Some(addr) = p["scoredEmailAddresses"][0]["address"].as_str() {
                        push(
                            Address {
                                name,
                                address: addr.to_string(),
                            },
                            &mut out,
                        );
                    }
                }
            }
        }
        for a in self.store.search_addresses(q, 10)? {
            push(a, &mut out);
        }
        out.truncate(10);
        Ok(out)
    }
}

/// Marker appended to data URIs that stand in for `cid:` references while
/// editing; a fragment does not affect how the image is decoded.
const CID_MARK: &str = "#cid=";

pub(crate) fn cid_to_data(html: &str, cid: &str, content_type: &str, b64: &str) -> String {
    html.replace(
        &format!("cid:{cid}"),
        &format!("data:{content_type};base64,{b64}{CID_MARK}{}", urlencoding::encode(cid)),
    )
}

/// Turns editor data URIs back into `cid:` references (the inline attachment
/// stays on the draft).
pub(crate) fn data_to_cid(html: &str) -> String {
    use std::sync::OnceLock;
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| regex::Regex::new(r#"data:[^"'\s>#]*#cid=([^"'\s>]+)"#).expect("regex"));
    re.replace_all(html, |caps: &regex::Captures| {
        let cid = urlencoding::decode(&caps[1]).map(|c| c.into_owned()).unwrap_or_default();
        format!("cid:{cid}")
    })
    .into_owned()
}

#[cfg(test)]
mod tests {
    use super::{cid_to_data, data_to_cid, prepend_to_body};

    #[test]
    fn cid_roundtrip() {
        let html = r#"<p><img src="cid:image001.png@01D"> und <img src='cid:image001.png@01D'></p>"#;
        let edit = cid_to_data(html, "image001.png@01D", "image/png", "iVBORw0K");
        assert!(!edit.contains("cid:"));
        assert!(edit.contains("data:image/png;base64,iVBORw0K#cid=image001.png%4001D"));
        assert_eq!(data_to_cid(&edit), html);
        // Ordinary data URIs (pasted images) are left alone.
        let pasted = r#"<img src="data:image/png;base64,AAAA">"#;
        assert_eq!(data_to_cid(pasted), pasted);
    }

    #[test]
    fn prepends_after_body_tag() {
        assert_eq!(
            prepend_to_body("<html><BODY class=x><p>a</p></body></html>", "<i>s</i>"),
            "<html><BODY class=x><i>s</i><p>a</p></body></html>"
        );
        assert_eq!(prepend_to_body("<p>a</p>", "<i>s</i>"), "<i>s</i><p>a</p>");
    }
}
