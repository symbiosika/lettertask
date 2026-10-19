use std::sync::Arc;

use base64::Engine;
use serde_json::{json, Value};

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

fn signature_block(sig: &str) -> String {
    if sig.trim().is_empty() {
        "<div><br></div>".into()
    } else {
        format!("<div><br></div><div class=\"signature\">{sig}</div>")
    }
}

impl Service {
    pub(crate) async fn create_draft(&self, mailbox_id: &str, mode: &str, source: Option<&str>) -> Result<Draft> {
        let sig = self.config().signature_html;
        if mode == "new" {
            return Ok(Draft {
                id: None,
                mailbox_id: mailbox_id.to_string(),
                mode: "new".into(),
                to: vec![],
                cc: vec![],
                bcc: vec![],
                subject: String::new(),
                body_html: signature_block(&sig),
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
        draft.body_html = prepend_to_body(&draft.body_html, &signature_block(&sig));
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
        let attachments = if v["hasAttachments"].as_bool().unwrap_or(false)
            || convert::body_html(&v).contains("cid:")
        {
            c.get_all(
                &format!(
                    "{}/messages/{}/attachments?$select=id,name,contentType,size,isInline",
                    mb.graph_prefix(),
                    seg(id)
                ),
                &[],
                500,
            )
            .await?
            .iter()
            .map(convert::attachment)
            .collect()
        } else {
            vec![]
        };
        Ok(Draft {
            id: Some(id.to_string()),
            mailbox_id: mb.id.clone(),
            mode: "new".into(),
            to: convert::addresses(&v, "toRecipients"),
            cc: convert::addresses(&v, "ccRecipients"),
            bcc: convert::addresses(&v, "bccRecipients"),
            subject: v["subject"].as_str().unwrap_or("").to_string(),
            body_html: convert::body_html(&v),
            importance: v["importance"].as_str().unwrap_or("normal").to_string(),
            request_read_receipt: v["isReadReceiptRequested"].as_bool().unwrap_or(false),
            attachments,
        })
    }

    pub(crate) async fn get_draft(&self, mailbox_id: &str, message_id: &str) -> Result<Draft> {
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
        if d.to.is_empty() && d.cc.is_empty() && d.bcc.is_empty() {
            return Err(Error::invalid("Bitte mindestens einen Empfänger angeben."));
        }
        let (mb, c) = self.mailbox(&d.mailbox_id)?;
        let id = self.write_draft(&mb, &c, &d).await?;
        c.post_empty(&format!("{}/messages/{}/send", mb.graph_prefix(), seg(&id)))
            .await?;
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

#[cfg(test)]
mod tests {
    use super::prepend_to_body;

    #[test]
    fn prepends_after_body_tag() {
        assert_eq!(
            prepend_to_body("<html><BODY class=x><p>a</p></body></html>", "<i>s</i>"),
            "<html><BODY class=x><i>s</i><p>a</p></body></html>"
        );
        assert_eq!(prepend_to_body("<p>a</p>", "<i>s</i>"), "<i>s</i><p>a</p>");
    }
}
