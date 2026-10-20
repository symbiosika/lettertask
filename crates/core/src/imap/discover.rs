//! Finds IMAP/SMTP settings for an address like Thunderbird does: Mozilla
//! ISPDB, then the provider's own autoconfig file, then common host names.

use std::time::Duration;

use serde::Serialize;

use super::ServerSettings;

/// Overridable for tests.
pub const ISPDB_ENV: &str = "OUTLOOK_ISPDB_URL";
const ISPDB: &str = "https://autoconfig.thunderbird.net/v1.1";

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Discovered {
    pub imap: ServerSettings,
    pub smtp: ServerSettings,
    /// "ispdb" | "autoconfig" | "guess"
    pub source: String,
}

pub async fn discover(http: &reqwest::Client, email: &str) -> Discovered {
    let email = email.trim();
    let domain = email.rsplit('@').next().unwrap_or("").to_lowercase();
    // The domain goes into URLs: host name characters only.
    let valid = !domain.is_empty()
        && domain.len() <= 253
        && domain.contains('.')
        && domain.chars().all(|c| c.is_alphanumeric() || c == '.' || c == '-')
        && !domain.starts_with(['.', '-'])
        && !domain.contains("..");
    if !valid || !email.contains('@') {
        return guess(email, &domain);
    }
    let base = std::env::var(ISPDB_ENV).unwrap_or_else(|_| ISPDB.to_string());
    let urls = [
        (format!("{}/{}", base.trim_end_matches('/'), domain), "ispdb"),
        (
            format!(
                "https://autoconfig.{domain}/mail/config-v1.1.xml?emailaddress={}",
                urlencoding::encode(email)
            ),
            "autoconfig",
        ),
    ];
    for (url, source) in urls {
        let resp = http.get(&url).timeout(Duration::from_secs(6)).send().await;
        let Ok(resp) = resp else { continue };
        if !resp.status().is_success() {
            continue;
        }
        let Some(xml) = read_limited(resp).await else { continue };
        if let Some((imap, smtp)) = parse_autoconfig(&xml, email) {
            return Discovered {
                imap,
                smtp,
                source: source.into(),
            };
        }
    }
    guess(email, &domain)
}

/// Config files are small; never buffer an endless response.
const MAX_CONFIG: usize = 256 * 1024;

async fn read_limited(mut resp: reqwest::Response) -> Option<String> {
    if resp.content_length().is_some_and(|l| l as usize > MAX_CONFIG) {
        return None;
    }
    let mut buf = Vec::new();
    while let Some(chunk) = resp.chunk().await.ok()? {
        buf.extend_from_slice(&chunk);
        if buf.len() > MAX_CONFIG {
            return None;
        }
    }
    String::from_utf8(buf).ok()
}

fn guess(email: &str, domain: &str) -> Discovered {
    Discovered {
        imap: ServerSettings {
            host: format!("imap.{domain}"),
            port: 993,
            security: "ssl".into(),
            username: email.into(),
        },
        smtp: ServerSettings {
            host: format!("smtp.{domain}"),
            port: 587,
            security: "starttls".into(),
            username: email.into(),
        },
        source: "guess".into(),
    }
}

/// Minimal parser for the Thunderbird autoconfig format. Of several
/// `incomingServer type="imap"` / `outgoingServer type="smtp"` entries the
/// first encrypted one wins (a plain-text entry is only used if nothing
/// else is offered); malformed entries are skipped.
pub fn parse_autoconfig(xml: &str, email: &str) -> Option<(ServerSettings, ServerSettings)> {
    let xml = strip_comments(xml);
    let pick = |tag: &str, kind: &str| -> Option<ServerSettings> {
        let all: Vec<ServerSettings> = server_blocks(&xml, tag, kind).into_iter().filter_map(|b| server(b, email)).collect();
        all.iter().find(|s| s.security != "none").or(all.first()).cloned()
    };
    Some((pick("incomingServer", "imap")?, pick("outgoingServer", "smtp")?))
}

fn strip_comments(xml: &str) -> String {
    let mut out = String::with_capacity(xml.len());
    let mut rest = xml;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        match rest[start..].find("-->") {
            Some(end) => rest = &rest[start + end + 3..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

fn server_blocks<'a>(xml: &'a str, tag: &str, kind: &str) -> Vec<&'a str> {
    let mut out = Vec::new();
    let mut rest = xml;
    let open_tag = format!("<{tag}");
    let close_tag = format!("</{tag}>");
    static TYPE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = TYPE.get_or_init(|| regex::Regex::new(r#"\stype\s*=\s*["']?([A-Za-z0-9]+)"#).expect("regex"));
    while let Some(start) = rest.find(&open_tag) {
        let after = &rest[start..];
        let Some(open_end) = after.find('>') else { break };
        let open = &after[..open_end];
        let Some(close) = after.find(&close_tag) else { break };
        if close < open_end {
            rest = &after[close + close_tag.len()..];
            continue;
        }
        let attr_type = re.captures(open).map(|c| c[1].to_ascii_lowercase());
        if attr_type.as_deref() == Some(kind) {
            out.push(&after[open_end + 1..close]);
        }
        rest = &after[close + close_tag.len()..];
    }
    out
}

fn element(block: &str, name: &str) -> Option<String> {
    let start = block.find(&format!("<{name}>"))? + name.len() + 2;
    let end = block[start..].find(&format!("</{name}>"))? + start;
    let v = block[start..end].trim();
    let v = v.strip_prefix("<![CDATA[").and_then(|v| v.strip_suffix("]]>")).unwrap_or(v);
    Some(html_escape::decode_html_entities(v.trim()).into_owned())
}

fn server(block: &str, email: &str) -> Option<ServerSettings> {
    let local = email.split('@').next().unwrap_or(email);
    let domain = email.rsplit('@').next().unwrap_or("");
    let fill = |s: &str| {
        s.replace("%EMAILADDRESS%", email)
            .replace("%EMAILLOCALPART%", local)
            .replace("%EMAILDOMAIN%", domain)
    };
    let host = fill(&element(block, "hostname")?);
    let host_ok = !host.is_empty()
        && host.len() <= 253
        && host.chars().all(|c| c.is_alphanumeric() || c == '.' || c == '-' || c == ':' || c == '[' || c == ']');
    if !host_ok {
        return None;
    }
    let port: u16 = element(block, "port")?.parse().ok().filter(|p| *p != 0)?;
    let security = match element(block, "socketType").unwrap_or_else(|| "SSL".into()).to_ascii_uppercase().as_str() {
        "SSL" | "TLS" => "ssl",
        "STARTTLS" => "starttls",
        "PLAIN" => "none",
        _ => return None,
    };
    let username = fill(&element(block, "username").unwrap_or_else(|| "%EMAILADDRESS%".into()));
    Some(ServerSettings {
        host,
        port,
        security: security.into(),
        username,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = r#"<?xml version="1.0"?>
<clientConfig version="1.1"><emailProvider id="example.org">
  <incomingServer type="pop3"><hostname>pop.example.org</hostname><port>995</port><socketType>SSL</socketType></incomingServer>
  <incomingServer type="imap">
    <hostname>imap.example.org</hostname><port>993</port><socketType>SSL</socketType>
    <username>%EMAILLOCALPART%</username><authentication>password-cleartext</authentication>
  </incomingServer>
  <outgoingServer type="smtp">
    <hostname>smtp.example.org</hostname><port>587</port><socketType>STARTTLS</socketType>
    <username>%EMAILADDRESS%</username>
  </outgoingServer>
</emailProvider></clientConfig>"#;

    #[test]
    fn parses_autoconfig() {
        let (i, s) = parse_autoconfig(XML, "anna@example.org").unwrap();
        assert_eq!((i.host.as_str(), i.port, i.security.as_str(), i.username.as_str()), ("imap.example.org", 993, "ssl", "anna"));
        assert_eq!((s.host.as_str(), s.port, s.security.as_str(), s.username.as_str()), ("smtp.example.org", 587, "starttls", "anna@example.org"));
    }

    #[test]
    fn autoconfig_edge_cases() {
        let e = "max@firma.de";
        // Malformed / incomplete files never panic and yield nothing.
        for xml in [
            "",
            "<clientConfig>",
            "<incomingServer type=\"imap\"><hostname>x</hostname>",
            "<incomingServer type=\"imap\" <hostname>",
            "<incomingServer type=\"imap\"><hostname>imap.x.de</hostname><port>abc</port></incomingServer><outgoingServer type=\"smtp\"><hostname>s</hostname><port>25</port></outgoingServer>",
            "<incomingServer type=\"imap\"><port>993</port></incomingServer><outgoingServer type=\"smtp\"><hostname>s</hostname><port>25</port></outgoingServer>",
            "<incomingServer type=\"imap\"><hostname>a</hostname><port>99999</port></incomingServer>",
            "</incomingServer><incomingServer type=\"imap\">",
            "<incomingServer type=\"imap\"><hostname>ä</hostname><port>1</port></incomingServer>\u{0}\u{ffff}",
        ] {
            assert_eq!(parse_autoconfig(xml, e), None, "{xml}");
        }
        // Several servers: encrypted beats plain, placeholders and entities
        // are resolved, comments and odd quoting are fine, broken entries skipped.
        let xml = r#"<clientConfig>
<!-- <incomingServer type="imap"><hostname>kommentar.de</hostname><port>1</port></incomingServer> -->
<incomingServer type="imap"><hostname>kaputt</hostname><port>x</port></incomingServer>
<incomingServer type='imap'><hostname>plain.%EMAILDOMAIN%</hostname><port>143</port><socketType>plain</socketType></incomingServer>
<incomingServer  type = "IMAP" ><hostname>imap.%EMAILDOMAIN%</hostname><port>993</port><socketType>ssl</socketType>
  <username><![CDATA[%EMAILLOCALPART%]]></username></incomingServer>
<outgoingServer type="smtp"><hostname>smtp.firma.de</hostname><port>587</port><socketType>STARTTLS</socketType>
  <username>a&amp;b</username></outgoingServer>
<outgoingServer type="smtp"><hostname>smtp2.firma.de</hostname><port>465</port><socketType>SSL</socketType></outgoingServer>
</clientConfig>"#;
        let (i, s) = parse_autoconfig(xml, e).unwrap();
        assert_eq!((i.host.as_str(), i.port, i.security.as_str(), i.username.as_str()), ("imap.firma.de", 993, "ssl", "max"));
        assert_eq!((s.host.as_str(), s.security.as_str(), s.username.as_str()), ("smtp.firma.de", "starttls", "a&b"));
        // Only plain offered: used as such (the user sees "none").
        let xml = r#"<incomingServer type="imap"><hostname>h.de</hostname><port>143</port><socketType>plain</socketType></incomingServer>
<outgoingServer type="smtp"><hostname>h.de</hostname><port>25</port><socketType>plain</socketType></outgoingServer>"#;
        let (i, _) = parse_autoconfig(xml, e).unwrap();
        assert_eq!(i.security, "none");
        // Unknown socket types are not mistaken for plain text.
        let xml = r#"<incomingServer type="imap"><hostname>h.de</hostname><port>1</port><socketType>QUANTUM</socketType></incomingServer>
<outgoingServer type="smtp"><hostname>h.de</hostname><port>25</port></outgoingServer>"#;
        assert_eq!(parse_autoconfig(xml, e), None);
        // Host names with injected URL/command characters are refused.
        let xml = r#"<incomingServer type="imap"><hostname>evil.de/x y</hostname><port>1</port></incomingServer>
<outgoingServer type="smtp"><hostname>h.de</hostname><port>25</port></outgoingServer>"#;
        assert_eq!(parse_autoconfig(xml, e), None);
    }

    #[tokio::test]
    async fn odd_addresses_fall_back_to_guess() {
        let http = reqwest::Client::new();
        for email in ["kein-at", "a@", "a@localhost", "a@evil.de/pfad?x=", "a@..de", "a@-x.de"] {
            assert_eq!(discover(&http, email).await.source, "guess", "{email}");
        }
    }

    #[test]
    fn guesses() {
        let d = guess("x@firma.de", "firma.de");
        assert_eq!(d.imap.host, "imap.firma.de");
        assert_eq!(d.smtp.port, 587);
    }
}
