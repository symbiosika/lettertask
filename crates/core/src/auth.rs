//! Microsoft identity platform login: authorization code flow with PKCE and a
//! loopback redirect (`http://localhost:<port>`), as recommended for public
//! desktop clients. Register the app in Entra ID as "Mobile and desktop
//! application" with redirect URI `http://localhost`.

use std::time::Duration;

use base64::Engine;
use chrono::{DateTime, Utc};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;

use crate::error::{Error, Result};

pub const DEFAULT_AUTHORITY: &str = "https://login.microsoftonline.com";

pub const SCOPES: &[&str] = &[
    "offline_access",
    "openid",
    "profile",
    "User.Read",
    "Mail.ReadWrite",
    "Mail.ReadWrite.Shared",
    "Mail.Send",
    "Mail.Send.Shared",
    "Calendars.ReadWrite",
    "Calendars.ReadWrite.Shared",
    "MailboxSettings.ReadWrite",
    "People.Read",
];

#[derive(Debug, Clone)]
pub struct OAuthConfig {
    /// e.g. `https://login.microsoftonline.com`
    pub authority: String,
    pub tenant: String,
    pub client_id: String,
}

impl OAuthConfig {
    fn endpoint(&self, name: &str) -> String {
        format!(
            "{}/{}/oauth2/v2.0/{}",
            self.authority.trim_end_matches('/'),
            self.tenant,
            name
        )
    }

    fn scope_string() -> String {
        SCOPES.join(" ")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenSet {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: DateTime<Utc>,
}

impl TokenSet {
    /// True when the access token expires within the next two minutes.
    pub fn needs_refresh(&self) -> bool {
        self.expires_at - chrono::Duration::seconds(120) <= Utc::now()
    }
}

pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

impl Pkce {
    pub fn generate() -> Self {
        let mut bytes = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut bytes);
        let verifier = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
        let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(Sha256::digest(verifier.as_bytes()));
        Pkce {
            verifier,
            challenge,
        }
    }
}

pub fn random_state() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

pub fn authorize_url(cfg: &OAuthConfig, redirect_uri: &str, state: &str, challenge: &str) -> String {
    let mut url = url::Url::parse(&cfg.endpoint("authorize")).expect("valid authority");
    url.query_pairs_mut()
        .append_pair("client_id", &cfg.client_id)
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("response_mode", "query")
        .append_pair("scope", &OAuthConfig::scope_string())
        .append_pair("state", state)
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("prompt", "select_account");
    url.to_string()
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
}

#[derive(Deserialize)]
struct TokenError {
    #[serde(default)]
    error: String,
    #[serde(default)]
    error_description: String,
}

async fn token_request(
    http: &reqwest::Client,
    cfg: &OAuthConfig,
    form: &[(&str, &str)],
    previous_refresh: Option<&str>,
) -> Result<TokenSet> {
    let resp = http.post(cfg.endpoint("token")).form(form).send().await?;
    let status = resp.status();
    let body = resp.text().await?;
    if !status.is_success() {
        let err: TokenError = serde_json::from_str(&body).unwrap_or(TokenError {
            error: status.to_string(),
            error_description: body.clone(),
        });
        let desc = err.error_description.lines().next().unwrap_or("").to_string();
        return Err(Error::Auth(format!("{} {}", err.error, desc).trim().to_string()));
    }
    let tr: TokenResponse = serde_json::from_str(&body)
        .map_err(|e| Error::Auth(format!("Ungültige Token-Antwort: {e}")))?;
    let refresh_token = tr
        .refresh_token
        .or_else(|| previous_refresh.map(str::to_string))
        .ok_or_else(|| Error::Auth("Kein Refresh-Token erhalten (offline_access fehlt?)".into()))?;
    Ok(TokenSet {
        access_token: tr.access_token,
        refresh_token,
        expires_at: Utc::now() + chrono::Duration::seconds(tr.expires_in.unwrap_or(3600)),
    })
}

pub async fn exchange_code(
    http: &reqwest::Client,
    cfg: &OAuthConfig,
    code: &str,
    verifier: &str,
    redirect_uri: &str,
) -> Result<TokenSet> {
    let scope = OAuthConfig::scope_string();
    token_request(
        http,
        cfg,
        &[
            ("client_id", cfg.client_id.as_str()),
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("code_verifier", verifier),
            ("scope", scope.as_str()),
        ],
        None,
    )
    .await
}

pub async fn refresh(http: &reqwest::Client, cfg: &OAuthConfig, refresh_token: &str) -> Result<TokenSet> {
    let scope = OAuthConfig::scope_string();
    token_request(
        http,
        cfg,
        &[
            ("client_id", cfg.client_id.as_str()),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("scope", scope.as_str()),
        ],
        Some(refresh_token),
    )
    .await
}

/// Device authorization grant: the user enters `user_code` at
/// `verification_uri` on any device. Works where no local browser redirect is
/// possible (remote sessions, terminals).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    #[serde(default = "default_expires")]
    pub expires_in: u64,
    #[serde(default = "default_interval")]
    pub interval: u64,
    #[serde(default)]
    pub message: String,
}

fn default_expires() -> u64 {
    900
}
fn default_interval() -> u64 {
    5
}

pub async fn start_device_code(http: &reqwest::Client, cfg: &OAuthConfig) -> Result<DeviceCode> {
    if cfg.client_id.trim().is_empty() {
        return Err(Error::Auth("Keine Client-ID konfiguriert.".into()));
    }
    let scope = OAuthConfig::scope_string();
    let resp = http
        .post(cfg.endpoint("devicecode"))
        .form(&[("client_id", cfg.client_id.as_str()), ("scope", scope.as_str())])
        .send()
        .await?;
    let status = resp.status();
    let body = resp.text().await?;
    if !status.is_success() {
        let err: TokenError = serde_json::from_str(&body).unwrap_or(TokenError {
            error: status.to_string(),
            error_description: body.clone(),
        });
        let desc = err.error_description.lines().next().unwrap_or("").to_string();
        return Err(Error::Auth(format!("{} {}", err.error, desc).trim().to_string()));
    }
    serde_json::from_str(&body).map_err(|e| Error::Auth(format!("Ungültige Device-Code-Antwort: {e}")))
}

/// Polls the token endpoint until the user completed the device login.
pub async fn poll_device_code(http: &reqwest::Client, cfg: &OAuthConfig, dc: &DeviceCode) -> Result<TokenSet> {
    let deadline = std::time::Instant::now() + Duration::from_secs(dc.expires_in);
    let mut interval = dc.interval.max(1);
    loop {
        tokio::time::sleep(Duration::from_secs(interval)).await;
        if std::time::Instant::now() > deadline {
            return Err(Error::Auth("Der Anmeldecode ist abgelaufen.".into()));
        }
        let res = token_request(
            http,
            cfg,
            &[
                ("client_id", cfg.client_id.as_str()),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("device_code", dc.device_code.as_str()),
            ],
            None,
        )
        .await;
        match res {
            Ok(t) => return Ok(t),
            Err(Error::Auth(msg)) if msg.starts_with("authorization_pending") => continue,
            Err(Error::Auth(msg)) if msg.starts_with("slow_down") => interval += 5,
            Err(e) => return Err(e),
        }
    }
}

const SUCCESS_PAGE: &str = "<!doctype html><html lang=\"de\"><head><meta charset=\"utf-8\"><title>Anmeldung</title></head>\
<body style=\"font-family:Segoe UI,sans-serif;padding:40px\"><h2>Anmeldung erfolgreich</h2>\
<p>Sie können dieses Browserfenster jetzt schließen und zur Anwendung zurückkehren.</p></body></html>";

const ERROR_PAGE: &str = "<!doctype html><html lang=\"de\"><head><meta charset=\"utf-8\"><title>Anmeldung</title></head>\
<body style=\"font-family:Segoe UI,sans-serif;padding:40px\"><h2>Anmeldung fehlgeschlagen</h2>\
<p>Bitte schließen Sie dieses Fenster und versuchen Sie es erneut.</p></body></html>";

/// Result of a single redirect hit on the loopback listener.
type Callback = std::result::Result<(String, String), String>;

/// Listens on 127.0.0.1 and (best effort) [::1] with the same port, because
/// browsers may resolve `localhost` to either.
pub struct LoopbackListener {
    pub port: u16,
    rx: mpsc::Receiver<Callback>,
}

impl LoopbackListener {
    pub async fn bind() -> Result<Self> {
        let v4 = TcpListener::bind(("127.0.0.1", 0)).await?;
        let port = v4.local_addr()?.port();
        let (tx, rx) = mpsc::channel(4);
        tokio::spawn(accept_loop(v4, tx.clone()));
        if let Ok(v6) = TcpListener::bind(("::1", port)).await {
            tokio::spawn(accept_loop(v6, tx));
        }
        Ok(LoopbackListener { port, rx })
    }

    pub fn redirect_uri(&self) -> String {
        format!("http://localhost:{}", self.port)
    }

    /// Waits for the redirect carrying `code` and checks `state`.
    pub async fn wait_for_code(&mut self, expected_state: &str, timeout: Duration) -> Result<String> {
        let res = tokio::time::timeout(timeout, self.rx.recv())
            .await
            .map_err(|_| Error::Auth("Zeitüberschreitung bei der Anmeldung".into()))?
            .ok_or_else(|| Error::Auth("Anmeldung abgebrochen".into()))?;
        match res {
            Ok((code, state)) if state == expected_state => Ok(code),
            Ok(_) => Err(Error::Auth("Ungültiger state-Parameter".into())),
            Err(e) => Err(Error::Auth(e)),
        }
    }
}

async fn accept_loop(listener: TcpListener, tx: mpsc::Sender<Callback>) {
    loop {
        let Ok((mut sock, _)) = listener.accept().await else {
            return;
        };
        let mut buf = vec![0u8; 8192];
        let n = match sock.read(&mut buf).await {
            Ok(n) => n,
            Err(_) => continue,
        };
        let req = String::from_utf8_lossy(&buf[..n]).to_string();
        let target = req
            .lines()
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .unwrap_or("/")
            .to_string();
        let parsed = url::Url::parse(&format!("http://localhost{target}")).ok();
        let params: std::collections::HashMap<String, String> = parsed
            .as_ref()
            .map(|u| u.query_pairs().into_owned().collect())
            .unwrap_or_default();
        let result: Option<Callback> = if let Some(code) = params.get("code") {
            Some(Ok((code.clone(), params.get("state").cloned().unwrap_or_default())))
        } else {
            params.get("error").map(|err| {
                Err(format!(
                    "{} {}",
                    err,
                    params.get("error_description").cloned().unwrap_or_default()
                ))
            })
        };
        let (status, page) = match &result {
            Some(Ok(_)) => ("200 OK", SUCCESS_PAGE),
            Some(Err(_)) => ("400 Bad Request", ERROR_PAGE),
            None => ("404 Not Found", ""),
        };
        let resp = format!(
            "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{page}",
            page.len()
        );
        let _ = sock.write_all(resp.as_bytes()).await;
        let _ = sock.shutdown().await;
        if let Some(r) = result {
            let _ = tx.send(r).await;
        }
    }
}

/// Full interactive login. `open_browser` must open the given URL for the user.
pub async fn interactive_login(
    http: &reqwest::Client,
    cfg: &OAuthConfig,
    open_browser: impl FnOnce(String) -> Result<()>,
    timeout: Duration,
) -> Result<TokenSet> {
    if cfg.client_id.trim().is_empty() {
        return Err(Error::Auth(
            "Keine Client-ID konfiguriert. Bitte unter Einstellungen die Client-ID der Azure App-Registrierung eintragen.".into(),
        ));
    }
    let mut listener = LoopbackListener::bind().await?;
    let redirect = listener.redirect_uri();
    let pkce = Pkce::generate();
    let state = random_state();
    open_browser(authorize_url(cfg, &redirect, &state, &pkce.challenge))?;
    let code = listener.wait_for_code(&state, timeout).await?;
    exchange_code(http, cfg, &code, &pkce.verifier, &redirect).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_challenge_is_sha256_of_verifier() {
        let p = Pkce::generate();
        let expect = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(Sha256::digest(p.verifier.as_bytes()));
        assert_eq!(p.challenge, expect);
        assert!(p.verifier.len() >= 43);
    }

    #[test]
    fn authorize_url_contains_required_params() {
        let cfg = OAuthConfig {
            authority: "https://login.example".into(),
            tenant: "organizations".into(),
            client_id: "abc".into(),
        };
        let u = authorize_url(&cfg, "http://localhost:1234", "st", "ch");
        let parsed = url::Url::parse(&u).unwrap();
        assert_eq!(parsed.path(), "/organizations/oauth2/v2.0/authorize");
        let q: std::collections::HashMap<_, _> = parsed.query_pairs().into_owned().collect();
        assert_eq!(q["client_id"], "abc");
        assert_eq!(q["code_challenge_method"], "S256");
        assert_eq!(q["redirect_uri"], "http://localhost:1234");
        assert!(q["scope"].contains("offline_access"));
        assert!(q["scope"].contains("Mail.ReadWrite"));
    }

    #[tokio::test]
    async fn device_code_flow_against_fake() {
        let fake = graph_fake::spawn(graph_fake::FakeOptions { seed_demo: false, now: Utc::now() }).await;
        let cfg = OAuthConfig {
            authority: fake.base_url.clone(),
            tenant: "organizations".into(),
            client_id: "test-client".into(),
        };
        let http = reqwest::Client::new();
        let dc = start_device_code(&http, &cfg).await.unwrap();
        assert!(!dc.user_code.is_empty());
        let t = poll_device_code(&http, &cfg, &dc).await.unwrap();
        assert!(!t.access_token.is_empty());
        assert!(!t.refresh_token.is_empty());
        let me: serde_json::Value = http
            .get(format!("{}/v1.0/me", fake.base_url))
            .bearer_auth(&t.access_token)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(me["mail"], "max.mustermann@contoso.example");
    }

    #[tokio::test]
    async fn loopback_receives_code() {
        let mut l = LoopbackListener::bind().await.unwrap();
        let port = l.port;
        tokio::spawn(async move {
            let _ = reqwest::get(format!("http://127.0.0.1:{port}/?code=XYZ&state=s1")).await;
        });
        let code = l.wait_for_code("s1", Duration::from_secs(5)).await.unwrap();
        assert_eq!(code, "XYZ");
    }

    #[tokio::test]
    async fn loopback_rejects_wrong_state_and_reports_errors() {
        let mut l = LoopbackListener::bind().await.unwrap();
        let port = l.port;
        tokio::spawn(async move {
            let _ = reqwest::get(format!("http://127.0.0.1:{port}/?code=XYZ&state=other")).await;
        });
        assert!(l.wait_for_code("s1", Duration::from_secs(5)).await.is_err());

        let mut l = LoopbackListener::bind().await.unwrap();
        let port = l.port;
        tokio::spawn(async move {
            let _ = reqwest::get(format!(
                "http://127.0.0.1:{port}/?error=access_denied&error_description=nope"
            ))
            .await;
        });
        let e = l.wait_for_code("s1", Duration::from_secs(5)).await.unwrap_err();
        assert!(e.to_string().contains("access_denied"));
    }
}
