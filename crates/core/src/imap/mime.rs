//! MIME parsing (mail-parser) and building (lettre) for IMAP accounts.

use base64::Engine;
use chrono::{DateTime, SecondsFormat, TimeZone, Utc};
use lettre::message::header::{ContentType, HeaderName, HeaderValue};
use lettre::message::{Attachment as LAttachment, Mailbox as LMailbox, MultiPart, SinglePart};
use mail_parser::{MessageParser, MimeHeaders};

use crate::convert;
use crate::error::{Error, Result};
use crate::model::{Address, Attachment, MessageDetail, MessageSummary};

pub fn parse(raw: &[u8]) -> Option<mail_parser::Message<'_>> {
    MessageParser::default().parse(raw)
}

pub fn addresses(a: Option<&mail_parser::Address<'_>>) -> Vec<Address> {
    let Some(a) = a else { return vec![] };
    a.iter()
        .filter_map(|x| {
            let address = x.address()?.to_string();
            Some(Address {
                name: x.name().unwrap_or("").to_string(),
                address,
            })
        })
        .collect()
}

fn ts(dt: &mail_parser::DateTime) -> Option<String> {
    Utc.timestamp_opt(dt.to_timestamp(), 0)
        .single()
        .map(|d| d.to_rfc3339_opts(SecondsFormat::Secs, true))
}

pub fn importance(msg: &mail_parser::Message<'_>) -> String {
    let prio = msg.header_raw("X-Priority").unwrap_or("").trim().chars().next();
    let imp = msg.header_raw("Importance").unwrap_or("").trim().to_ascii_lowercase();
    match (prio, imp.as_str()) {
        (Some('1' | '2'), _) | (_, "high") => "high".into(),
        (Some('4' | '5'), _) | (_, "low") => "low".into(),
        _ => "normal".into(),
    }
}

/// Plain-text preview from HTML or text (single line, ~200 chars).
pub fn preview_text(msg: &mail_parser::Message<'_>) -> String {
    let text = msg
        .body_text(0)
        .map(|t| t.into_owned())
        .or_else(|| msg.body_html(0).map(|h| html_to_text(&h)))
        .unwrap_or_default();
    let collapsed: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed.chars().take(200).collect()
}

pub fn html_to_text(html: &str) -> String {
    use std::sync::OnceLock;
    static BLOCKS: OnceLock<regex::Regex> = OnceLock::new();
    static TAGS: OnceLock<regex::Regex> = OnceLock::new();
    let blocks = BLOCKS.get_or_init(|| regex::Regex::new(r"(?is)<(style|script|head)[^>]*>.*?</(style|script|head)>").expect("regex"));
    let tags = TAGS.get_or_init(|| regex::Regex::new(r"(?s)<[^>]*>").expect("regex"));
    let s = blocks.replace_all(html, " ");
    let s = tags.replace_all(&s, " ");
    html_escape::decode_html_entities(&s).into_owned()
}

/// Message flags as strings (`\Seen`, keywords, …).
pub fn flag_strings<'a>(flags: impl Iterator<Item = async_imap::types::Flag<'a>>) -> Vec<String> {
    use async_imap::types::Flag;
    flags
        .map(|f| match f {
            Flag::Seen => "\\Seen".into(),
            Flag::Answered => "\\Answered".into(),
            Flag::Flagged => "\\Flagged".into(),
            Flag::Deleted => "\\Deleted".into(),
            Flag::Draft => "\\Draft".into(),
            Flag::Recent => "\\Recent".into(),
            Flag::MayCreate => "\\*".into(),
            Flag::Custom(c) => c.into_owned(),
        })
        .collect()
}

pub fn has_flag(flags: &[String], f: &str) -> bool {
    flags.iter().any(|x| x.eq_ignore_ascii_case(f))
}

pub fn flag_status(flags: &[String]) -> String {
    if has_flag(flags, "\\Flagged") {
        "flagged".into()
    } else if has_flag(flags, "$Completed") {
        "complete".into()
    } else {
        "notFlagged".into()
    }
}

pub fn categories(flags: &[String]) -> Vec<String> {
    flags.iter().filter_map(|f| super::keyword_category(f)).collect()
}

/// Message summary from the header (+ beginning of the body for the preview).
#[allow(clippy::too_many_arguments)]
pub fn summary(
    mailbox_id: &str,
    folder: &str,
    id: String,
    header: &[u8],
    text: Option<&[u8]>,
    flags: &[String],
    internal_date: Option<DateTime<chrono::FixedOffset>>,
) -> MessageSummary {
    let mut raw = header.to_vec();
    if let Some(t) = text {
        raw.extend_from_slice(t);
    }
    let parsed = parse(&raw);
    let (subject, from, to, date, importance, preview, has_att, thread) = match &parsed {
        Some(m) => {
            let ct = m
                .root_part()
                .content_type()
                .map(|c| format!("{}/{}", c.ctype(), c.subtype().unwrap_or("")).to_ascii_lowercase())
                .unwrap_or_default();
            (
                m.subject().unwrap_or("").to_string(),
                addresses(m.from()).into_iter().next(),
                addresses(m.to()),
                m.date().and_then(ts),
                self::importance(m),
                preview_text(m),
                ct == "multipart/mixed",
                thread_id(m).unwrap_or_default(),
            )
        }
        None => Default::default(),
    };
    let received = internal_date
        .map(|d| d.with_timezone(&Utc).to_rfc3339_opts(SecondsFormat::Secs, true))
        .or(date)
        .unwrap_or_default();
    MessageSummary {
        id,
        mailbox_id: mailbox_id.to_string(),
        folder_id: folder.to_string(),
        conversation_id: thread,
        subject,
        from,
        to,
        received_at: received,
        is_read: has_flag(flags, "\\Seen"),
        is_draft: has_flag(flags, "\\Draft"),
        importance,
        flag: flag_status(flags),
        has_attachments: has_att,
        categories: categories(flags),
        preview,
        due_at: due(flags),
        recurrence: crate::model::recurrence_of(&categories(flags)),
    }
}

/// Due time from a `$due_…` keyword.
pub fn due(flags: &[String]) -> Option<String> {
    flags.iter().find_map(|f| super::keyword_due(f))
}

fn first_msg_id(v: &str) -> Option<String> {
    use std::sync::OnceLock;
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| regex::Regex::new(r"<([^<>\s]+)>").expect("regex"));
    re.captures(v).map(|c| c[1].to_string())
}

/// Thread root: first id in References, else In-Reply-To, else the own
/// Message-ID (all without angle brackets).
pub fn thread_id(m: &mail_parser::Message<'_>) -> Option<String> {
    m.header_raw("References")
        .and_then(first_msg_id)
        .or_else(|| m.header_raw("In-Reply-To").and_then(first_msg_id))
        .or_else(|| m.message_id().map(str::to_string))
}

/// Own Message-ID (without brackets) from raw header bytes.
pub fn message_id_of(header: &[u8]) -> Option<String> {
    parse(header).and_then(|m| m.message_id().map(str::to_string))
}

/// Attachments of a parsed message: (index, part). The index is the stable
/// attachment id used by the UI.
fn attachments<'a>(msg: &'a mail_parser::Message<'a>) -> impl Iterator<Item = (usize, &'a mail_parser::MessagePart<'a>)> {
    msg.attachments().enumerate()
}

fn part_name(p: &mail_parser::MessagePart<'_>, idx: usize) -> String {
    p.attachment_name()
        .map(str::to_string)
        .unwrap_or_else(|| {
            if p.is_message() {
                format!("nachricht-{}.eml", idx + 1)
            } else {
                format!("anhang-{}", idx + 1)
            }
        })
}

fn part_type(p: &mail_parser::MessagePart<'_>) -> String {
    p.content_type()
        .map(|c| match c.subtype() {
            Some(s) => format!("{}/{}", c.ctype(), s),
            None => c.ctype().to_string(),
        })
        .unwrap_or_else(|| "application/octet-stream".into())
}

fn part_bytes(p: &mail_parser::MessagePart<'_>) -> Vec<u8> {
    p.contents().to_vec()
}

pub fn content_id(p: &mail_parser::MessagePart<'_>) -> Option<String> {
    p.content_id()
        .map(|c| c.trim().trim_matches(['<', '>']).to_string())
        .filter(|c| !c.is_empty())
}

/// HTML body (text converted) of a parsed message.
pub fn html_body(msg: &mail_parser::Message<'_>) -> String {
    if msg.html_body_count() > 0 {
        if let Some(h) = msg.body_html(0) {
            return h.into_owned();
        }
    }
    msg.body_text(0)
        .map(|t| convert::text_to_html(&t))
        .unwrap_or_default()
}

/// Inline images referenced by the HTML as (cid, content type, bytes).
pub fn inline_images(msg: &mail_parser::Message<'_>, html: &str) -> Vec<(String, String, Vec<u8>)> {
    attachments(msg)
        .filter_map(|(_, p)| {
            let cid = content_id(p)?;
            html.contains(&format!("cid:{cid}"))
                .then(|| (cid, part_type(p), part_bytes(p)))
        })
        .collect()
}

/// Full message → MessageDetail; `cid:` images become data: URIs and are not
/// listed as attachments.
pub fn detail(raw: &[u8], mut summary: MessageSummary) -> Result<MessageDetail> {
    let msg = parse(raw).ok_or_else(|| Error::Other("Nachricht konnte nicht gelesen werden".into()))?;
    let mut body = html_body(&msg);
    let mut list = Vec::new();
    for (idx, p) in attachments(&msg) {
        if let Some(cid) = content_id(p) {
            let needle = format!("cid:{cid}");
            if body.contains(&needle) {
                let data = format!(
                    "data:{};base64,{}",
                    part_type(p),
                    base64::engine::general_purpose::STANDARD.encode(p.contents())
                );
                body = body.replace(&needle, &data);
                continue;
            }
        }
        list.push(Attachment {
            id: idx.to_string(),
            name: part_name(p, idx),
            content_type: part_type(p),
            size: p.contents().len() as i64,
            is_inline: false,
            content_id: content_id(p),
        });
    }
    summary.has_attachments = !list.is_empty();
    if summary.subject.is_empty() {
        summary.subject = msg.subject().unwrap_or("").to_string();
    }
    Ok(MessageDetail {
        cc: addresses(msg.cc()),
        bcc: addresses(msg.bcc()),
        reply_to: addresses(msg.reply_to()),
        sent_at: msg.date().and_then(ts),
        body_html: body,
        attachments: list,
        internet_message_id: msg.message_id().map(|m| format!("<{m}>")),
        web_link: None,
        is_meeting_request: false,
        event_id: None,
        summary,
    })
}

/// Bytes of attachment `idx`: (file name, content type, bytes).
pub fn attachment(raw: &[u8], idx: usize) -> Result<(String, String, Vec<u8>)> {
    let msg = parse(raw).ok_or_else(|| Error::Other("Nachricht konnte nicht gelesen werden".into()))?;
    let (i, p) = attachments(&msg)
        .find(|(i, _)| *i == idx)
        .ok_or_else(|| Error::NotFound("Anhang".into()))?;
    Ok((part_name(p, i), part_type(p), part_bytes(p)))
}

/// An attachment part: (index, file name, content type, bytes, content id).
pub type Part = (usize, String, String, Vec<u8>, Option<String>);

/// All attachment parts (for forwarding / keeping draft attachments).
pub fn all_attachments(raw: &[u8]) -> Vec<Part> {
    let Some(msg) = parse(raw) else { return vec![] };
    attachments(&msg)
        .map(|(i, p)| (i, part_name(p, i), part_type(p), part_bytes(p), content_id(p)))
        .collect()
}

/// Everything needed to build a MIME message.
#[derive(Debug, Clone, Default)]
pub struct Outgoing {
    pub from: Address,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    pub bcc: Vec<Address>,
    pub reply_to: Vec<Address>,
    pub subject: String,
    pub html: String,
    pub importance: String,
    pub read_receipt: bool,
    pub message_id: String,
    pub in_reply_to: Option<String>,
    pub references: Option<String>,
    /// (name, content type, bytes)
    pub attachments: Vec<(String, String, Vec<u8>)>,
    /// (cid, content type, bytes) referenced from the HTML
    pub inline: Vec<(String, String, Vec<u8>)>,
}

fn mailbox(a: &Address) -> Result<LMailbox> {
    let addr = a
        .address
        .trim()
        .parse()
        .map_err(|_| Error::invalid(format!("Ungültige Adresse: {}", a.address)))?;
    let name = a.name.trim();
    Ok(LMailbox::new((!name.is_empty()).then(|| name.to_string()), addr))
}

fn content_type(ct: &str) -> ContentType {
    ContentType::parse(ct).unwrap_or_else(|_| ContentType::parse("application/octet-stream").expect("ct"))
}

/// Builds the message. `draft` keeps Bcc in the headers and allows empty
/// recipient lists.
pub fn build(o: &Outgoing, draft: bool) -> Result<lettre::Message> {
    let from = mailbox(&o.from)?;
    let mut b = lettre::Message::builder()
        .from(from.clone())
        .subject(o.subject.clone())
        .date_now()
        .message_id(Some(o.message_id.clone()))
        .user_agent(concat!("lettertask/", env!("CARGO_PKG_VERSION")).to_string());
    for a in &o.to {
        b = b.to(mailbox(a)?);
    }
    for a in &o.cc {
        b = b.cc(mailbox(a)?);
    }
    for a in &o.bcc {
        b = b.bcc(mailbox(a)?);
    }
    for a in &o.reply_to {
        b = b.reply_to(mailbox(a)?);
    }
    if let Some(r) = &o.in_reply_to {
        b = b.in_reply_to(r.clone());
    }
    if let Some(r) = &o.references {
        b = b.references(r.clone());
    }
    match o.importance.as_str() {
        "high" => {
            b = b
                .raw_header(HeaderValue::new(HeaderName::new_from_ascii_str("X-Priority"), "1".into()))
                .raw_header(HeaderValue::new(HeaderName::new_from_ascii_str("Importance"), "high".into()));
        }
        "low" => {
            b = b
                .raw_header(HeaderValue::new(HeaderName::new_from_ascii_str("X-Priority"), "5".into()))
                .raw_header(HeaderValue::new(HeaderName::new_from_ascii_str("Importance"), "low".into()));
        }
        _ => {}
    }
    if o.read_receipt {
        b = b.raw_header(HeaderValue::new(
            HeaderName::new_from_ascii_str("Disposition-Notification-To"),
            o.from.address.clone(),
        ));
    }
    if draft {
        b = b.keep_bcc();
        if o.to.is_empty() && o.cc.is_empty() && o.bcc.is_empty() {
            let env = lettre::address::Envelope::new(Some(from.email.clone()), vec![from.email.clone()])
                .map_err(|e| Error::invalid(e.to_string()))?;
            b = b.envelope(env);
        }
    }
    let html_part = SinglePart::html(o.html.clone());
    let body = if o.inline.is_empty() {
        None
    } else {
        let mut rel = MultiPart::related().singlepart(html_part.clone());
        for (cid, ct, bytes) in &o.inline {
            rel = rel.singlepart(LAttachment::new_inline(cid.clone()).body(bytes.clone(), content_type(ct)));
        }
        Some(rel)
    };
    let msg = if o.attachments.is_empty() {
        match body {
            Some(rel) => b.multipart(rel),
            None => b.singlepart(html_part),
        }
    } else {
        let mut mixed = match body {
            Some(rel) => MultiPart::mixed().multipart(rel),
            None => MultiPart::mixed().singlepart(html_part),
        };
        for (name, ct, bytes) in &o.attachments {
            mixed = mixed.singlepart(LAttachment::new(name.clone()).body(bytes.clone(), content_type(ct)));
        }
        b.multipart(mixed)
    };
    msg.map_err(|e| Error::invalid(format!("Nachricht konnte nicht erstellt werden: {e}")))
}

/// New unique Message-ID for `from_address`'s domain.
pub fn new_message_id(from_address: &str) -> String {
    let domain = from_address.rsplit('@').next().filter(|d| !d.is_empty()).unwrap_or("localhost");
    format!("<{}@{}>", uuid::Uuid::new_v4().simple(), domain)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(n: &str, x: &str) -> Address {
        Address {
            name: n.into(),
            address: x.into(),
        }
    }

    #[test]
    fn build_and_parse_roundtrip() {
        let o = Outgoing {
            from: a("Max", "max@example.org"),
            to: vec![a("Anna Groß", "anna@example.org")],
            bcc: vec![a("", "geheim@example.org")],
            subject: "Grüße – Test".into(),
            html: "<p>Hallo <img src=\"cid:logo1\"></p>".into(),
            importance: "high".into(),
            message_id: new_message_id("max@example.org"),
            in_reply_to: Some("<orig@example.org>".into()),
            attachments: vec![("bericht.txt".into(), "text/plain".into(), b"inhalt".to_vec())],
            inline: vec![("logo1".into(), "image/png".into(), vec![137, 80, 78, 71])],
            ..Default::default()
        };
        let raw = build(&o, true).unwrap().formatted();
        let msg = parse(&raw).unwrap();
        assert_eq!(msg.subject(), Some("Grüße – Test"));
        assert_eq!(addresses(msg.bcc())[0].address, "geheim@example.org");
        assert_eq!(importance(&msg), "high");
        let s = summary("mb", "INBOX", "x".into(), &raw, None, &["\\Seen".into(), "$cat_Info".into()], None);
        assert!(s.has_attachments);
        assert_eq!(s.categories, ["Info"]);
        let d = detail(&raw, s).unwrap();
        assert!(d.body_html.contains("data:image/png;base64,"));
        assert_eq!(d.attachments.len(), 1);
        assert_eq!(d.attachments[0].name, "bericht.txt");
        let (name, _, bytes) = attachment(&raw, d.attachments[0].id.parse().unwrap()).unwrap();
        assert_eq!((name.as_str(), bytes.as_slice()), ("bericht.txt", b"inhalt".as_slice()));
        // Sending drops Bcc from the headers.
        let sent = build(&o, false).unwrap().formatted();
        assert!(addresses(parse(&sent).unwrap().bcc()).is_empty());
    }

    #[test]
    fn draft_without_recipients_builds() {
        let o = Outgoing {
            from: a("", "max@example.org"),
            subject: "Entwurf".into(),
            message_id: new_message_id("max@example.org"),
            ..Default::default()
        };
        assert!(build(&o, true).is_ok());
    }
}
