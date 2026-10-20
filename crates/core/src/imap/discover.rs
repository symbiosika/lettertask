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
    if domain.is_empty() || !email.contains('@') {
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
        let Ok(xml) = resp.text().await else { continue };
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

/// Minimal parser for the Thunderbird autoconfig format: takes the first
/// `incomingServer type="imap"` and the first `outgoingServer type="smtp"`.
pub fn parse_autoconfig(xml: &str, email: &str) -> Option<(ServerSettings, ServerSettings)> {
    let imap = server_block(xml, "incomingServer", "imap").and_then(|b| server(b, email))?;
    let smtp = server_block(xml, "outgoingServer", "smtp").and_then(|b| server(b, email))?;
    Some((imap, smtp))
}

fn server_block<'a>(xml: &'a str, tag: &str, kind: &str) -> Option<&'a str> {
    let mut rest = xml;
    while let Some(start) = rest.find(&format!("<{tag}")) {
        let after = &rest[start..];
        let open_end = after.find('>')?;
        let open = &after[..open_end];
        let close = after.find(&format!("</{tag}>"))?;
        if open.contains(&format!("type=\"{kind}\"")) || open.contains(&format!("type='{kind}'")) {
            return Some(&after[open_end + 1..close]);
        }
        rest = &after[close..];
    }
    None
}

fn element<'a>(block: &'a str, name: &str) -> Option<&'a str> {
    let start = block.find(&format!("<{name}>"))? + name.len() + 2;
    let end = block[start..].find(&format!("</{name}>"))? + start;
    Some(block[start..end].trim())
}

fn server(block: &str, email: &str) -> Option<ServerSettings> {
    let host = element(block, "hostname")?.to_string();
    let port: u16 = element(block, "port")?.parse().ok()?;
    let security = match element(block, "socketType").unwrap_or("SSL") {
        "SSL" => "ssl",
        "STARTTLS" => "starttls",
        _ => "none",
    };
    let local = email.split('@').next().unwrap_or(email);
    let domain = email.rsplit('@').next().unwrap_or("");
    let username = element(block, "username")
        .unwrap_or("%EMAILADDRESS%")
        .replace("%EMAILADDRESS%", email)
        .replace("%EMAILLOCALPART%", local)
        .replace("%EMAILDOMAIN%", domain);
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
    fn guesses() {
        let d = guess("x@firma.de", "firma.de");
        assert_eq!(d.imap.host, "imap.firma.de");
        assert_eq!(d.smtp.port, 587);
    }
}
