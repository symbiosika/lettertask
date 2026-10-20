//! IMAP/SMTP support for non-Microsoft mailboxes: connection handling,
//! settings discovery and MIME helpers. The service layer (service/imap_*.rs)
//! maps it onto the same API the Graph accounts use.

pub mod conn;
pub mod discover;
pub mod mime;

use serde::{Deserialize, Serialize};

/// Server endpoint of an IMAP or SMTP server.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ServerSettings {
    pub host: String,
    pub port: u16,
    /// "ssl" (implicit TLS), "starttls" or "none".
    pub security: String,
    pub username: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ImapAccountInput {
    pub email: String,
    #[serde(default)]
    pub display_name: String,
    pub imap: ServerSettings,
    pub smtp: ServerSettings,
    #[serde(default)]
    pub password: String,
    /// `None` = same password as IMAP.
    #[serde(default)]
    pub smtp_password: Option<String>,
}

/// Account ids of IMAP accounts carry this prefix; the primary mailbox shares the id.
pub const ID_PREFIX: &str = "imap:";

pub fn is_imap_id(id: &str) -> bool {
    id.starts_with(ID_PREFIX)
}

pub fn account_id_for(email: &str) -> String {
    format!("{ID_PREFIX}{}", email.trim().to_lowercase())
}

/// Message id: `{uidvalidity}:{uid}:{folder}` (folder last, it may contain ':').
pub fn message_id(uid_validity: u32, uid: u32, folder: &str) -> String {
    format!("{uid_validity}:{uid}:{folder}")
}

pub fn parse_message_id(id: &str) -> Option<(u32, u32, &str)> {
    let mut it = id.splitn(3, ':');
    let v = it.next()?.parse().ok()?;
    let u = it.next()?.parse().ok()?;
    let f = it.next()?;
    Some((v, u, f))
}

/// Categories are stored as IMAP keywords: `$cat_` + the name with every
/// character outside `[A-Za-z0-9_-]` written as `.XXXX` (UTF-16 hex, so the
/// keyword stays a valid IMAP atom).
pub fn category_keyword(name: &str) -> String {
    let mut out = String::from("$cat_");
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            out.push(ch);
        } else {
            let mut buf = [0u16; 2];
            for unit in ch.encode_utf16(&mut buf) {
                out.push_str(&format!(".{unit:04x}"));
            }
        }
    }
    out
}

pub fn keyword_category(keyword: &str) -> Option<String> {
    let rest = keyword.strip_prefix("$cat_")?;
    let mut units: Vec<u16> = Vec::new();
    let mut out = String::new();
    let mut chars = rest.chars().peekable();
    let flush = |units: &mut Vec<u16>, out: &mut String| {
        if !units.is_empty() {
            out.push_str(&String::from_utf16_lossy(units));
            units.clear();
        }
    };
    while let Some(c) = chars.next() {
        if c == '.' {
            let hex: String = chars.by_ref().take(4).collect();
            units.push(u16::from_str_radix(&hex, 16).ok()?);
        } else {
            flush(&mut units, &mut out);
            out.push(c);
        }
    }
    flush(&mut units, &mut out);
    Some(out)
}

/// Quotes a string for IMAP commands (quoted-string; 8-bit is passed through,
/// which servers accept with CHARSET UTF-8).
pub fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            '\r' | '\n' => out.push(' '),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Compact UID set like `1:3,7,9:10`.
pub fn uid_set(uids: &[u32]) -> String {
    let mut v = uids.to_vec();
    v.sort_unstable();
    v.dedup();
    let mut parts = Vec::new();
    let mut i = 0;
    while i < v.len() {
        let start = v[i];
        let mut end = start;
        while i + 1 < v.len() && v[i + 1] == end + 1 {
            i += 1;
            end = v[i];
        }
        parts.push(if start == end {
            start.to_string()
        } else {
            format!("{start}:{end}")
        });
        i += 1;
    }
    parts.join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn category_keywords_roundtrip() {
        for name in ["Wichtig", "Business & Pitch", "Rechnung 2026", "Grüße 😀", "a.b"] {
            let k = category_keyword(name);
            assert!(k.chars().all(|c| c.is_ascii_alphanumeric() || "$_-.".contains(c)), "{k}");
            assert_eq!(keyword_category(&k).as_deref(), Some(name));
        }
        assert_eq!(keyword_category("\\Seen"), None);
    }

    #[test]
    fn ids_and_sets() {
        let id = message_id(7, 42, "INBOX/a:b");
        assert_eq!(parse_message_id(&id), Some((7, 42, "INBOX/a:b")));
        assert_eq!(uid_set(&[5, 1, 2, 3, 9, 10, 12]), "1:3,5,9:10,12");
        assert_eq!(quote("a\"b\\c"), "\"a\\\"b\\\\c\"");
    }
}
