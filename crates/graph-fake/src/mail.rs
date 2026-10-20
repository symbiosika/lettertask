//! Mail folders, messages, delta, attachments (incl. upload sessions), send.

use axum::body::Bytes;
use axum::extract::{Path, State as AxState};
use axum::http::{header, HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Duration, Utc};
use serde_json::{json, Value};

use crate::state::{message_json, Att, Folder, Mailbox, Msg, SharedState, State, Upload};
use crate::util::{
    apply_select, b64url, created, email, email_addr, escape_html, iso, json_resp, merge, new_id, no_content, ok, paged,
    parse_utc, preview, unb64, unb64url, GErr, GResult, Req,
};

const MAX_INLINE_ATTACHMENT: usize = 3 * 1024 * 1024;

pub fn handle(s: &mut State, idx: usize, rest: &[&str], req: &Req) -> GResult {
    let m = &req.method;
    match rest {
        ["mailFolders"] if m == Method::GET => {
            let mb = &s.mailboxes[idx];
            let root = mb.root_id.clone();
            list_folders(mb, &root, req)
        }
        ["mailFolders"] if m == Method::POST => {
            let root = s.mailboxes[idx].root_id.clone();
            create_folder(&mut s.mailboxes[idx], &root, req)
        }
        ["mailFolders", fid] => folder_item(&mut s.mailboxes[idx], fid, req),
        ["mailFolders", fid, "childFolders"] => {
            let mb = &mut s.mailboxes[idx];
            let id = folder(mb, fid)?.id;
            if m == Method::POST {
                create_folder(mb, &id, req)
            } else {
                list_folders(mb, &id, req)
            }
        }
        ["mailFolders", fid, "messages"] if m == Method::GET => {
            let mb = &s.mailboxes[idx];
            let id = folder(mb, fid)?.id;
            list_messages(mb, Some(&id), req)
        }
        ["mailFolders", fid, "messages"] if m == Method::POST => {
            let mb = &mut s.mailboxes[idx];
            let id = folder(mb, fid)?.id;
            create_message(mb, &id, req)
        }
        ["mailFolders", fid, "messages", "delta"] if m == Method::GET => {
            let mb = &s.mailboxes[idx];
            let id = folder(mb, fid)?.id;
            delta(mb, &id, req)
        }
        ["messages"] if m == Method::GET => list_messages(&s.mailboxes[idx], None, req),
        ["messages"] if m == Method::POST => {
            let mb = &mut s.mailboxes[idx];
            let drafts = mb.folder_id("drafts").unwrap_or_default();
            create_message(mb, &drafts, req)
        }
        ["messages", id] => message_item(&mut s.mailboxes[idx], id, req),
        ["messages", id, "$value"] if m == Method::GET => mime(&s.mailboxes[idx], id),
        ["messages", id, "attachments"] => attachments(&mut s.mailboxes[idx], id, req),
        ["messages", id, "attachments", "createUploadSession"] if m == Method::POST => create_upload_session(s, idx, id, req),
        ["messages", id, "attachments", aid] => attachment_item(&mut s.mailboxes[idx], id, aid, req),
        ["messages", id, "attachments", aid, "$value"] if m == Method::GET => attachment_value(&s.mailboxes[idx], id, aid),
        ["messages", id, action] if m == Method::POST => message_action(s, idx, id, action, req),
        ["sendMail"] if m == Method::POST => send_mail(s, idx, req),
        _ => Err(GErr::new(
            StatusCode::BAD_REQUEST,
            "BadRequest",
            format!("Unsupported request: {} /{}", req.method, rest.join("/")),
        )),
    }
}

fn folder(mb: &Mailbox, key: &str) -> Result<Folder, GErr> {
    mb.folder(key).cloned().ok_or_else(|| GErr::not_found("mailFolder"))
}

fn msg<'a>(mb: &'a Mailbox, id: &str) -> Result<&'a Msg, GErr> {
    mb.messages.get(id).ok_or_else(|| GErr::not_found("message"))
}

// ---------------------------------------------------------------- folders

fn list_folders(mb: &Mailbox, parent: &str, req: &Req) -> GResult {
    let mut items: Vec<&Folder> = mb.folders.iter().filter(|f| f.parent_id == parent).collect();
    items.sort_by_key(|f| f.display_name.to_lowercase());
    let vals: Vec<Value> = items.into_iter().map(|f| apply_select(mb.render_folder(f), req.q("select"))).collect();
    ok(paged(req, vals, 10))
}

fn create_folder(mb: &mut Mailbox, parent: &str, req: &Req) -> GResult {
    let body = req.json()?;
    let name = body["displayName"].as_str().unwrap_or("").trim().to_string();
    if name.is_empty() {
        return Err(GErr::bad("displayName is required."));
    }
    if mb.folders.iter().any(|f| f.parent_id == parent && f.display_name.eq_ignore_ascii_case(&name)) {
        return Err(GErr::new(StatusCode::CONFLICT, "ErrorFolderExists", "A folder with the specified name already exists."));
    }
    let f = Folder { id: new_id("AAMkAF"), display_name: name, parent_id: parent.to_string(), well_known: None };
    mb.folders.push(f.clone());
    created(mb.render_folder(&f))
}

fn is_under(mb: &Mailbox, folder_id: &str, ancestor: &str) -> bool {
    let mut cur = folder_id.to_string();
    for _ in 0..64 {
        if cur == ancestor {
            return true;
        }
        match mb.folders.iter().find(|f| f.id == cur) {
            Some(f) => cur = f.parent_id.clone(),
            None => return false,
        }
    }
    false
}

fn folder_item(mb: &mut Mailbox, key: &str, req: &Req) -> GResult {
    let f = folder(mb, key)?;
    match req.method {
        Method::GET => ok(apply_select(mb.render_folder(&f), req.q("select"))),
        Method::PATCH => {
            let body = req.json()?;
            if let Some(name) = body["displayName"].as_str() {
                if let Some(ff) = mb.folders.iter_mut().find(|x| x.id == f.id) {
                    ff.display_name = name.to_string();
                }
            }
            let f = folder(mb, &f.id)?;
            ok(mb.render_folder(&f))
        }
        Method::DELETE => {
            if f.well_known.is_some() {
                return Err(GErr::new(StatusCode::BAD_REQUEST, "ErrorDeleteDistinguishedFolder", "Distinguished folders cannot be deleted."));
            }
            let deleted = mb.folder_id("deleteditems").unwrap_or_default();
            if is_under(mb, &f.id, &deleted) {
                mb.delete_folder(&f.id);
            } else if let Some(ff) = mb.folders.iter_mut().find(|x| x.id == f.id) {
                // Like Outlook: a deleted folder moves into "Deleted Items".
                ff.parent_id = deleted;
            }
            no_content()
        }
        _ => Err(GErr::bad("Unsupported method")),
    }
}

// ---------------------------------------------------------------- messages

#[derive(Default, Clone, Debug)]
struct Filter {
    ge: Option<DateTime<Utc>>,
    gt: Option<DateTime<Utc>>,
    lt: Option<DateTime<Utc>>,
    le: Option<DateTime<Utc>>,
    is_read: Option<bool>,
    flag: Option<String>,
}

impl Filter {
    fn parse(s: Option<&str>) -> Result<Filter, GErr> {
        let mut f = Filter::default();
        let Some(s) = s else { return Ok(f) };
        for clause in s.split(" and ") {
            let parts: Vec<&str> = clause.split_whitespace().collect();
            if parts.len() != 3 {
                return Err(unsupported_filter(s));
            }
            let (prop, op, val) = (parts[0], parts[1].to_ascii_lowercase(), parts[2].trim_matches('\''));
            match prop {
                "receivedDateTime" => {
                    let dt = parse_utc(val).ok_or_else(|| GErr::bad(format!("Invalid filter date '{val}'")))?;
                    match op.as_str() {
                        "ge" => f.ge = Some(dt),
                        "gt" => f.gt = Some(dt),
                        "lt" => f.lt = Some(dt),
                        "le" => f.le = Some(dt),
                        _ => return Err(unsupported_filter(s)),
                    }
                }
                "isRead" if op == "eq" => f.is_read = Some(val == "true"),
                "flag/flagStatus" if op == "eq" => f.flag = Some(val.to_string()),
                _ => return Err(unsupported_filter(s)),
            }
        }
        Ok(f)
    }

    fn matches(&self, m: &Msg) -> bool {
        self.ge.is_none_or(|d| m.received >= d)
            && self.gt.is_none_or(|d| m.received > d)
            && self.lt.is_none_or(|d| m.received < d)
            && self.le.is_none_or(|d| m.received <= d)
            && self.is_read.is_none_or(|r| m.is_read() == r)
            && self.flag.as_deref().is_none_or(|fl| m.json["flag"]["flagStatus"].as_str() == Some(fl))
    }
}

fn unsupported_filter(s: &str) -> GErr {
    GErr::new(
        StatusCode::BAD_REQUEST,
        "ErrorInvalidUrlQueryFilter",
        format!("The query filter '{s}' is not supported by graph-fake (supported: receivedDateTime ge/gt/lt/le, isRead eq, flag/flagStatus eq, joined with 'and')."),
    )
}

/// KQL subset: bare words (subject/body/from/to/attachment names), and
/// `from:` `to:` `subject:` `body:` `hasAttachments:true|false` restrictions.
fn search_matches(m: &Msg, terms: &[String]) -> bool {
    let j = &m.json;
    let text = |p: &str| j.pointer(p).and_then(Value::as_str).unwrap_or("").to_lowercase();
    let recipients = || {
        let mut s = String::new();
        for key in ["toRecipients", "ccRecipients"] {
            for r in j[key].as_array().into_iter().flatten() {
                s.push_str(&email_addr(r));
                s.push(' ');
                s.push_str(&r.pointer("/emailAddress/name").and_then(Value::as_str).unwrap_or("").to_lowercase());
                s.push(' ');
            }
        }
        s
    };
    let from = format!("{} {}", text("/from/emailAddress/name"), text("/from/emailAddress/address"));
    let body = format!("{} {}", text("/bodyPreview"), text("/body/content"));
    let mut all = format!("{} {} {} {}", text("/subject"), body, from, recipients());
    for a in &m.attachments {
        all.push_str(&a.name.to_lowercase());
        all.push(' ');
    }
    terms.iter().all(|t| match t.split_once(':') {
        Some(("from", v)) => from.contains(v),
        Some(("to", v)) => recipients().contains(v),
        Some(("subject", v)) => text("/subject").contains(v),
        Some(("body", v)) => body.contains(v),
        Some(("hasattachments", v)) => (v == "true") == m.attachments.iter().any(|a| !a.is_inline),
        _ => all.contains(t.as_str()),
    })
}

fn render_msg(mb: &Mailbox, m: &Msg, req: &Req) -> Value {
    let mut v = apply_select(m.render(), req.q("select"));
    if let (Some(exp), Some(eid)) = (req.q("expand"), &m.event_id) {
        if exp.contains("event") && mb.events.contains_key(eid) {
            v["event"] = json!({ "@odata.etag": "W/\"evt\"", "id": eid });
        }
    }
    v
}

fn list_messages(mb: &Mailbox, folder_id: Option<&str>, req: &Req) -> GResult {
    let filter = Filter::parse(req.q("filter"))?;
    let terms: Vec<String> = req
        .q("search")
        .map(|s| s.trim().trim_matches('"').to_lowercase().split_whitespace().map(str::to_string).collect())
        .unwrap_or_default();
    let mut items: Vec<&Msg> = mb
        .messages
        .values()
        .filter(|m| folder_id.is_none_or(|f| m.folder_id == f))
        .filter(|m| filter.matches(m))
        .filter(|m| terms.is_empty() || search_matches(m, &terms))
        .collect();
    let asc = req.q("orderby").is_some_and(|o| o.to_lowercase().ends_with(" asc"));
    items.sort_by(|a, b| if asc { a.received.cmp(&b.received) } else { b.received.cmp(&a.received) }.then(a.id.cmp(&b.id)));
    let count = items.len();
    let vals: Vec<Value> = items.into_iter().map(|m| render_msg(mb, m, req)).collect();
    let mut out = paged(req, vals, 10);
    if req.q("count") == Some("true") {
        out["@odata.count"] = json!(count);
    }
    ok(out)
}

/// Properties a client may set when creating/updating a message.
const WRITABLE: &[&str] = &[
    "subject", "body", "toRecipients", "ccRecipients", "bccRecipients", "replyTo", "importance",
    "isReadReceiptRequested", "isDeliveryReceiptRequested", "categories", "flag", "isRead", "from", "sender",
    "internetMessageHeaders", "inferenceClassification",
];
/// Only updatable while the message is a draft.
const DRAFT_ONLY: &[&str] = &["body", "toRecipients", "ccRecipients", "bccRecipients", "replyTo", "from", "sender"];

fn parse_attachment(v: &Value) -> Result<Att, GErr> {
    let ty = v["@odata.type"].as_str().unwrap_or("#microsoft.graph.fileAttachment");
    if ty != "#microsoft.graph.fileAttachment" {
        return Err(GErr::bad(format!("graph-fake only supports fileAttachment, got {ty}")));
    }
    let name = v["name"].as_str().unwrap_or("").to_string();
    if name.is_empty() {
        return Err(GErr::bad("Attachment name is required."));
    }
    let bytes = unb64(v["contentBytes"].as_str().unwrap_or(""))
        .ok_or_else(|| GErr::bad("contentBytes must be base64 encoded."))?;
    if bytes.len() > MAX_INLINE_ATTACHMENT {
        return Err(GErr::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "ErrorRequestEntityTooLarge",
            "The attachment exceeds 3 MB; use attachments/createUploadSession.",
        ));
    }
    let ct = v["contentType"].as_str().filter(|s| !s.is_empty()).unwrap_or("application/octet-stream");
    Ok(Att::new(&name, ct, bytes, v["isInline"].as_bool().unwrap_or(false), v["contentId"].as_str()))
}

fn build_message(mb: &Mailbox, body: &Value, draft: bool) -> Result<(Value, Vec<Att>), GErr> {
    let now = Utc::now();
    let mut props = json!({});
    if let Some(obj) = body.as_object() {
        for (k, v) in obj {
            if WRITABLE.contains(&k.as_str()) {
                props[k] = v.clone();
            }
        }
    }
    if props.get("from").is_none() {
        props["from"] = mb.me();
    }
    props["isDraft"] = json!(draft);
    props["isRead"] = json!(true);
    let mut atts = Vec::new();
    for a in body["attachments"].as_array().into_iter().flatten() {
        atts.push(parse_attachment(a)?);
    }
    let json = message_json(now, &props);
    Ok((json, atts))
}

fn create_message(mb: &mut Mailbox, folder_id: &str, req: &Req) -> GResult {
    let body = req.json()?;
    let (json, atts) = build_message(mb, &body, true)?;
    let m = Msg { id: new_id("AAMkAD"), folder_id: folder_id.to_string(), json, received: Utc::now(), attachments: atts, event_id: None };
    let id = mb.insert(m);
    created(mb.messages[&id].render())
}

fn message_item(mb: &mut Mailbox, id: &str, req: &Req) -> GResult {
    match req.method {
        Method::GET => {
            let m = msg(mb, id)?;
            ok(render_msg(mb, m, req))
        }
        Method::PATCH => {
            let body = req.json()?;
            let m = mb.messages.get_mut(id).ok_or_else(|| GErr::not_found("message"))?;
            let is_draft = m.json["isDraft"].as_bool().unwrap_or(false);
            let obj = body.as_object().cloned().unwrap_or_default();
            for (k, v) in &obj {
                if !WRITABLE.contains(&k.as_str()) {
                    return Err(GErr::new(
                        StatusCode::BAD_REQUEST,
                        "ErrorInvalidPropertyRequest",
                        format!("The property '{k}' is read-only and can't be set."),
                    ));
                }
                if DRAFT_ONLY.contains(&k.as_str()) && !is_draft {
                    return Err(GErr::new(
                        StatusCode::BAD_REQUEST,
                        "ErrorInvalidPropertyRequest",
                        format!("The property '{k}' can only be updated on draft messages."),
                    ));
                }
                m.json[k] = v.clone();
            }
            if obj.contains_key("body") {
                m.json["bodyPreview"] = json!(preview(&m.json["body"]));
            }
            mb.changed(id);
            ok(mb.messages[id].render())
        }
        Method::DELETE => {
            mb.remove(id).ok_or_else(|| GErr::not_found("message"))?;
            no_content()
        }
        _ => Err(GErr::bad("Unsupported method")),
    }
}

fn recipients_html(v: &Value) -> String {
    v.as_array()
        .into_iter()
        .flatten()
        .map(|r| {
            let n = r.pointer("/emailAddress/name").and_then(Value::as_str).unwrap_or("");
            let a = r.pointer("/emailAddress/address").and_then(Value::as_str).unwrap_or("");
            if n.is_empty() || n == a {
                escape_html(a)
            } else {
                format!("{} &lt;{}&gt;", escape_html(n), escape_html(a))
            }
        })
        .collect::<Vec<_>>()
        .join("; ")
}

fn inner_body(body: &Value) -> String {
    let content = body["content"].as_str().unwrap_or("");
    if body["contentType"].as_str().unwrap_or("text").eq_ignore_ascii_case("html") {
        let lower = content.to_ascii_lowercase();
        if let Some(start) = lower.find("<body") {
            let after = lower[start..].find('>').map(|p| start + p + 1).unwrap_or(start);
            let end = lower.rfind("</body>").unwrap_or(content.len());
            if end >= after {
                return content[after..end].to_string();
            }
        }
        content.to_string()
    } else {
        format!("<div style=\"white-space:pre-wrap\">{}</div>", escape_html(content))
    }
}

/// Builds the createReply/createReplyAll/createForward draft.
fn reply_draft(mb: &mut Mailbox, id: &str, kind: &str, req: &Req) -> Result<String, GErr> {
    let orig = msg(mb, id)?.clone();
    let body = req.json()?;
    let me = mb.addr.clone();
    let j = &orig.json;
    let prefix = if kind == "createForward" { "WG: " } else { "AW: " };
    let subj = j["subject"].as_str().unwrap_or("");
    let lower = subj.to_lowercase();
    let subject = if lower.starts_with(&prefix.to_lowercase()) || (prefix == "AW: " && lower.starts_with("re: ")) {
        subj.to_string()
    } else {
        format!("{prefix}{subj}")
    };
    let from = if j["replyTo"].as_array().is_some_and(|a| !a.is_empty()) { j["replyTo"].clone() } else { json!([j["from"].clone()]) };
    let not_me = |r: &&Value| email_addr(r) != me;
    let (to, cc) = match kind {
        "createReply" => (from, json!([])),
        "createReplyAll" => {
            let mut to: Vec<Value> = from.as_array().cloned().unwrap_or_default();
            for r in j["toRecipients"].as_array().into_iter().flatten().filter(not_me) {
                if !to.iter().any(|t| email_addr(t) == email_addr(r)) {
                    to.push(r.clone());
                }
            }
            let mut to: Vec<Value> = to.into_iter().filter(|r| email_addr(r) != me).collect();
            if to.is_empty() {
                to = from.as_array().cloned().unwrap_or_default();
            }
            let cc: Vec<Value> = j["ccRecipients"].as_array().into_iter().flatten().filter(not_me).filter(|r| !to.iter().any(|t| email_addr(t) == email_addr(r))).cloned().collect();
            (json!(to), json!(cc))
        }
        _ => (body["toRecipients"].as_array().map(|a| json!(a)).unwrap_or(json!([])), json!([])),
    };
    let comment = body["comment"].as_str().map(escape_html).unwrap_or_default();
    let sent = j["sentDateTime"].as_str().and_then(parse_utc).unwrap_or(orig.received);
    let from_html = recipients_html(&json!([j["from"].clone()]));
    let mut header = format!(
        "<b>Von:</b> {from_html}<br><b>Gesendet:</b> {}<br><b>An:</b> {}<br>",
        sent.format("%d.%m.%Y %H:%M"),
        recipients_html(&j["toRecipients"])
    );
    if j["ccRecipients"].as_array().is_some_and(|a| !a.is_empty()) {
        header.push_str(&format!("<b>Cc:</b> {}<br>", recipients_html(&j["ccRecipients"])));
    }
    header.push_str(&format!("<b>Betreff:</b> {}", escape_html(subj)));
    let html = format!(
        "<html><head><meta http-equiv=\"Content-Type\" content=\"text/html; charset=utf-8\"></head><body>\
<div>{comment}</div><div><br></div>\
<hr style=\"display:inline-block;width:98%\" tabindex=\"-1\">\
<div id=\"divRplyFwdMsg\" dir=\"ltr\"><font face=\"Calibri, sans-serif\" style=\"font-size:11pt\" color=\"#000000\">{header}</font><div>&nbsp;</div></div>\
{}</body></html>",
        inner_body(&j["body"])
    );
    let mut props = json!({
        "subject": subject,
        "toRecipients": to,
        "ccRecipients": cc,
        "from": mb.me(),
        "isDraft": true,
        "isRead": true,
        "conversationId": j["conversationId"].clone(),
        "body": { "contentType": "html", "content": html },
    });
    // optional `message` with extra properties
    if let Some(extra) = body.get("message").and_then(Value::as_object) {
        for (k, v) in extra {
            if WRITABLE.contains(&k.as_str()) {
                props[k] = v.clone();
            }
        }
    }
    let attachments = if kind == "createForward" {
        orig.attachments.iter().map(Att::with_new_id).collect()
    } else {
        orig.attachments.iter().filter(|a| a.is_inline).map(Att::with_new_id).collect()
    };
    let drafts = mb.folder_id("drafts").unwrap_or_default();
    let m = Msg { id: new_id("AAMkAD"), folder_id: drafts, json: message_json(Utc::now(), &props), received: Utc::now(), attachments, event_id: None };
    Ok(mb.insert(m))
}

fn has_recipients(j: &Value) -> bool {
    ["toRecipients", "ccRecipients", "bccRecipients"]
        .iter()
        .any(|k| j[k].as_array().is_some_and(|a| a.iter().any(|r| !email_addr(r).is_empty())))
}

/// Sends a draft: moves it to Sent Items (new id) and delivers it.
fn send_draft(s: &mut State, idx: usize, id: &str) -> Result<(), GErr> {
    let mb = &mut s.mailboxes[idx];
    let m = msg(mb, id)?;
    if !m.json["isDraft"].as_bool().unwrap_or(false) {
        return Err(GErr::new(StatusCode::BAD_REQUEST, "ErrorInvalidOperation", "Only draft messages can be sent."));
    }
    if !has_recipients(&m.json) {
        return Err(GErr::new(StatusCode::BAD_REQUEST, "ErrorInvalidRecipients", "At least one recipient isn't valid."));
    }
    let sent_id = mb.folder_id("sentitems").unwrap_or_default();
    let mut m = mb.remove(id).expect("checked above");
    let now = Utc::now();
    m.id = new_id("AAMkAD");
    m.folder_id = sent_id;
    m.received = now;
    m.json["isDraft"] = json!(false);
    m.json["isRead"] = json!(true);
    m.json["sentDateTime"] = json!(iso(now));
    m.json["receivedDateTime"] = json!(iso(now));
    m.json["from"] = mb.me();
    m.json["sender"] = mb.me();
    m.touch();
    let copy = m.clone();
    mb.insert(m);
    s.deliver(&copy);
    Ok(())
}

fn message_action(s: &mut State, idx: usize, id: &str, action: &str, req: &Req) -> GResult {
    match action {
        "move" | "copy" => {
            let body = req.json()?;
            let dest_key = body["destinationId"].as_str().ok_or_else(|| GErr::bad("destinationId is required."))?;
            let mb = &mut s.mailboxes[idx];
            msg(mb, id)?;
            let dest = folder(mb, dest_key)?.id;
            let new_id = if action == "move" { mb.move_msg(id, &dest) } else { mb.copy_msg(id, &dest) }
                .ok_or_else(|| GErr::not_found("message"))?;
            created(mb.messages[&new_id].render())
        }
        "createReply" | "createReplyAll" | "createForward" => {
            let mb = &mut s.mailboxes[idx];
            let nid = reply_draft(mb, id, action, req)?;
            created(mb.messages[&nid].render())
        }
        "reply" | "replyAll" | "forward" => {
            let kind = match action {
                "reply" => "createReply",
                "replyAll" => "createReplyAll",
                _ => "createForward",
            };
            let nid = reply_draft(&mut s.mailboxes[idx], id, kind, req)?;
            send_draft(s, idx, &nid)?;
            crate::util::accepted()
        }
        "send" => {
            send_draft(s, idx, id)?;
            crate::util::accepted()
        }
        _ => Err(GErr::new(StatusCode::BAD_REQUEST, "BadRequest", format!("Unsupported action '{action}'"))),
    }
}

fn send_mail(s: &mut State, idx: usize, req: &Req) -> GResult {
    let body = req.json()?;
    let message = body.get("message").ok_or_else(|| GErr::bad("message is required."))?;
    let mb = &mut s.mailboxes[idx];
    let (mut json, atts) = build_message(mb, message, false)?;
    if !has_recipients(&json) {
        return Err(GErr::new(StatusCode::BAD_REQUEST, "ErrorInvalidRecipients", "At least one recipient isn't valid."));
    }
    let now = Utc::now();
    json["from"] = mb.me();
    json["sender"] = mb.me();
    let m = Msg { id: new_id("AAMkAD"), folder_id: mb.folder_id("sentitems").unwrap_or_default(), json, received: now, attachments: atts, event_id: None };
    if body["saveToSentItems"].as_bool().unwrap_or(true) {
        mb.insert(m.clone());
    }
    s.deliver(&m);
    crate::util::accepted()
}

// ---------------------------------------------------------------- delta

fn delta(mb: &Mailbox, folder_id: &str, req: &Req) -> GResult {
    let size = req.max_page_size().or(req.top()).unwrap_or(10).max(1);
    let decode = |t: &str| -> Result<Value, GErr> {
        unb64url(t)
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .ok_or_else(|| GErr::new(StatusCode::BAD_REQUEST, "SyncStateInvalid", "The sync state is invalid. Restart synchronization."))
    };
    let link = |token_key: &str, payload: Value| {
        req.link_with(
            &[(token_key, b64url(payload.to_string().as_bytes()))],
            &["$skiptoken", "$deltatoken", "$filter", "$select", "$top", "$orderby", "$expand"],
        )
    };

    if let Some(tok) = req.q("deltatoken") {
        let t = decode(tok)?;
        let since = t["s"].as_u64().unwrap_or(0);
        let select = t["sel"].as_str().map(str::to_string);
        let mut ids: Vec<&str> = Vec::new();
        for (seq, f, id) in &mb.changes {
            if *seq > since && f == folder_id && !ids.contains(&id.as_str()) {
                ids.push(id);
            }
        }
        let mut values = Vec::new();
        for id in ids {
            match mb.messages.get(id) {
                Some(m) if m.folder_id == folder_id => values.push(apply_select(m.render(), select.as_deref())),
                _ => values.push(json!({ "@odata.type": "#microsoft.graph.message", "id": id, "@removed": { "reason": "deleted" } })),
            }
        }
        // Incremental rounds are returned in one page (good enough for clients).
        return ok(json!({
            "@odata.context": format!("{}/v1.0/$metadata#Collection(message)", req.base_url),
            "value": values,
            "@odata.deltaLink": link("$deltatoken", json!({ "s": mb.seq, "sel": select })),
        }));
    }

    let (snapshot, offset, filter_s, select) = match req.q("skiptoken") {
        Some(tok) => {
            let t = decode(tok)?;
            (
                t["s"].as_u64().unwrap_or(0),
                t["o"].as_u64().unwrap_or(0) as usize,
                t["f"].as_str().map(str::to_string),
                t["sel"].as_str().map(str::to_string),
            )
        }
        None => (mb.seq, 0, req.q("filter").map(str::to_string), req.q("select").map(str::to_string)),
    };
    let filter = Filter::parse(filter_s.as_deref())?;
    let all: Vec<&Msg> = mb.folder_messages(folder_id).into_iter().filter(|m| filter.matches(m)).collect();
    let values: Vec<Value> = all.iter().skip(offset).take(size).map(|m| apply_select(m.render(), select.as_deref())).collect();
    let mut out = json!({
        "@odata.context": format!("{}/v1.0/$metadata#Collection(message)", req.base_url),
        "value": values,
    });
    if offset + size < all.len() {
        out["@odata.nextLink"] = json!(link("$skiptoken", json!({ "s": snapshot, "o": offset + size, "f": filter_s, "sel": select })));
    } else {
        out["@odata.deltaLink"] = json!(link("$deltatoken", json!({ "s": snapshot, "sel": select })));
    }
    ok(out)
}

// ---------------------------------------------------------------- attachments

fn attachments(mb: &mut Mailbox, id: &str, req: &Req) -> GResult {
    match req.method {
        Method::GET => {
            let m = msg(mb, id)?;
            let vals: Vec<Value> = m.attachments.iter().map(|a| apply_select(a.render(false), req.q("select"))).collect();
            ok(json!({ "@odata.context": format!("{}/v1.0/$metadata", req.base_url), "value": vals }))
        }
        Method::POST => {
            let att = parse_attachment(&req.json()?)?;
            let v = att.render(false);
            mb.messages.get_mut(id).ok_or_else(|| GErr::not_found("message"))?.attachments.push(att);
            mb.changed(id);
            created(v)
        }
        _ => Err(GErr::bad("Unsupported method")),
    }
}

fn attachment_item(mb: &mut Mailbox, id: &str, aid: &str, req: &Req) -> GResult {
    match req.method {
        Method::GET => {
            let m = msg(mb, id)?;
            let a = m.attachments.iter().find(|a| a.id == aid).ok_or_else(|| GErr::not_found("attachment"))?;
            ok(apply_select(a.render(true), req.q("select")))
        }
        Method::DELETE => {
            let m = mb.messages.get_mut(id).ok_or_else(|| GErr::not_found("message"))?;
            let before = m.attachments.len();
            m.attachments.retain(|a| a.id != aid);
            if m.attachments.len() == before {
                return Err(GErr::not_found("attachment"));
            }
            mb.changed(id);
            no_content()
        }
        _ => Err(GErr::bad("Unsupported method")),
    }
}

fn attachment_value(mb: &Mailbox, id: &str, aid: &str) -> GResult {
    let m = msg(mb, id)?;
    let a = m.attachments.iter().find(|a| a.id == aid).ok_or_else(|| GErr::not_found("attachment"))?;
    Ok((StatusCode::OK, [(header::CONTENT_TYPE, a.content_type.clone())], a.bytes.clone()).into_response())
}

fn create_upload_session(s: &mut State, idx: usize, id: &str, req: &Req) -> GResult {
    let body = req.json()?;
    let item = body.get("AttachmentItem").or_else(|| body.get("attachmentItem")).ok_or_else(|| GErr::bad("AttachmentItem is required."))?;
    if item["attachmentType"].as_str().unwrap_or("file") != "file" {
        return Err(GErr::bad("Only attachmentType 'file' is supported."));
    }
    let name = item["name"].as_str().filter(|n| !n.is_empty()).ok_or_else(|| GErr::bad("AttachmentItem.name is required."))?;
    let size = item["size"].as_u64().ok_or_else(|| GErr::bad("AttachmentItem.size is required."))? as usize;
    msg(&s.mailboxes[idx], id)?;
    let sid = uuid::Uuid::new_v4().simple().to_string();
    let expires = Utc::now() + Duration::hours(1);
    s.uploads.insert(
        sid.clone(),
        Upload {
            mailbox: s.mailboxes[idx].addr.clone(),
            message_id: id.to_string(),
            name: name.to_string(),
            content_type: item["contentType"].as_str().unwrap_or("application/octet-stream").to_string(),
            size,
            data: Vec::with_capacity(size),
            is_inline: item["isInline"].as_bool().unwrap_or(false),
            content_id: item["contentId"].as_str().map(str::to_string),
            expires,
        },
    );
    created(json!({
        "@odata.context": format!("{}/v1.0/$metadata#microsoft.graph.uploadSession", s.base_url),
        "uploadUrl": format!("{}/upload/{}", s.base_url, sid),
        "expirationDateTime": iso(expires),
        "nextExpectedRanges": ["0-"],
    }))
}

/// `PUT {uploadUrl}` – no Authorization header required (like the real service).
pub async fn upload_chunk(AxState(st): AxState<SharedState>, Path(sid): Path<String>, headers: HeaderMap, body: Bytes) -> Response {
    let mut s = st.lock();
    let Some(up) = s.uploads.get_mut(&sid) else {
        return GErr::new(StatusCode::NOT_FOUND, "ErrorItemNotFound", "Upload session not found or expired.").into_response();
    };
    let range = headers.get(header::CONTENT_RANGE).and_then(|v| v.to_str().ok()).unwrap_or("");
    let parsed = range.strip_prefix("bytes ").and_then(|r| {
        let (span, total) = r.split_once('/')?;
        let (a, b) = span.split_once('-')?;
        Some((a.parse::<usize>().ok()?, b.parse::<usize>().ok()?, total.parse::<usize>().ok()?))
    });
    let Some((a, b, total)) = parsed else {
        return GErr::bad("Content-Range header 'bytes a-b/total' is required.").into_response();
    };
    if total != up.size || a != up.data.len() || b < a || b - a + 1 != body.len() || b >= total {
        return GErr::new(
            StatusCode::RANGE_NOT_SATISFIABLE,
            "InvalidRange",
            format!("Expected range starting at {} of {}, got {range} with {} bytes.", up.data.len(), up.size, body.len()),
        )
        .into_response();
    }
    up.data.extend_from_slice(&body);
    if up.data.len() < up.size {
        let v = json!({ "expirationDateTime": iso(up.expires), "nextExpectedRanges": [format!("{}-", up.data.len())] });
        return json_resp(StatusCode::OK, v);
    }
    let up = s.uploads.remove(&sid).expect("present");
    let att = Att::new(&up.name, &up.content_type, up.data, up.is_inline, up.content_id.as_deref());
    let aid = att.id.clone();
    let base = s.base_url.clone();
    let Some(idx) = s.mailbox_idx(&up.mailbox) else {
        return GErr::not_found("mailbox").into_response();
    };
    let mb = &mut s.mailboxes[idx];
    match mb.messages.get_mut(&up.message_id) {
        Some(m) => m.attachments.push(att),
        None => return GErr::not_found("message").into_response(),
    }
    mb.changed(&up.message_id);
    let location = format!("{base}/v1.0/users/{}/messages/{}/attachments/{aid}", up.mailbox, up.message_id);
    (StatusCode::CREATED, [(header::LOCATION, location)]).into_response()
}

pub async fn upload_cancel(AxState(st): AxState<SharedState>, Path(sid): Path<String>) -> Response {
    st.lock().uploads.remove(&sid);
    StatusCode::NO_CONTENT.into_response()
}

// ---------------------------------------------------------------- MIME

fn mime(mb: &Mailbox, id: &str) -> GResult {
    let m = msg(mb, id)?;
    let j = &m.json;
    let addr = |v: &Value| -> String {
        v.as_array()
            .into_iter()
            .flatten()
            .map(|r| {
                let n = r.pointer("/emailAddress/name").and_then(Value::as_str).unwrap_or("");
                format!("\"{}\" <{}>", n, r.pointer("/emailAddress/address").and_then(Value::as_str).unwrap_or(""))
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    let date = j["sentDateTime"].as_str().and_then(parse_utc).unwrap_or(m.received);
    let mut out = String::new();
    out.push_str(&format!("From: {}\r\n", addr(&json!([j["from"].clone()]))));
    out.push_str(&format!("To: {}\r\n", addr(&j["toRecipients"])));
    if j["ccRecipients"].as_array().is_some_and(|a| !a.is_empty()) {
        out.push_str(&format!("Cc: {}\r\n", addr(&j["ccRecipients"])));
    }
    out.push_str(&format!("Subject: {}\r\n", j["subject"].as_str().unwrap_or("")));
    out.push_str(&format!("Date: {}\r\n", date.to_rfc2822()));
    out.push_str(&format!("Message-ID: {}\r\n", j["internetMessageId"].as_str().unwrap_or("")));
    out.push_str("MIME-Version: 1.0\r\n");
    let html = j["body"]["contentType"].as_str() == Some("html");
    let ct = if html { "text/html" } else { "text/plain" };
    let content = j["body"]["content"].as_str().unwrap_or("");
    if m.attachments.is_empty() {
        out.push_str(&format!("Content-Type: {ct}; charset=\"utf-8\"\r\nContent-Transfer-Encoding: 8bit\r\n\r\n{content}\r\n"));
    } else {
        let boundary = format!("_000_{}", uuid::Uuid::new_v4().simple());
        out.push_str(&format!("Content-Type: multipart/mixed; boundary=\"{boundary}\"\r\n\r\n"));
        out.push_str(&format!("--{boundary}\r\nContent-Type: {ct}; charset=\"utf-8\"\r\n\r\n{content}\r\n"));
        for a in &m.attachments {
            let b = crate::util::b64(&a.bytes);
            let lines: Vec<String> = b.as_bytes().chunks(76).map(|c| String::from_utf8_lossy(c).into_owned()).collect();
            out.push_str(&format!(
                "--{boundary}\r\nContent-Type: {}; name=\"{}\"\r\nContent-Disposition: {}; filename=\"{}\"\r\n{}Content-Transfer-Encoding: base64\r\n\r\n{}\r\n",
                a.content_type,
                a.name,
                if a.is_inline { "inline" } else { "attachment" },
                a.name,
                a.content_id.as_ref().map(|c| format!("Content-ID: <{c}>\r\n")).unwrap_or_default(),
                lines.join("\r\n")
            ));
        }
        out.push_str(&format!("--{boundary}--\r\n"));
    }
    Ok((StatusCode::OK, [(header::CONTENT_TYPE, "text/plain")], out).into_response())
}

/// Helper used by the seed: builds a received message.
#[allow(clippy::too_many_arguments)]
pub fn seed_msg(mb: &Mailbox, folder: &str, from: (&str, &str), subject: &str, html: &str, received: DateTime<Utc>, extra: Value) -> Msg {
    let mut props = json!({
        "from": email(from.0, from.1),
        "toRecipients": [mb.me()],
        "subject": subject,
        "body": { "contentType": "html", "content": html },
        "receivedDateTime": iso(received),
        "sentDateTime": iso(received - Duration::seconds(7)),
        "createdDateTime": iso(received),
        "lastModifiedDateTime": iso(received),
    });
    merge(&mut props, &extra);
    Msg {
        id: new_id("AAMkAD"),
        folder_id: mb.folder_id(folder).unwrap_or_else(|| folder.to_string()),
        json: message_json(received, &props),
        received,
        attachments: vec![],
        event_id: None,
    }
}
