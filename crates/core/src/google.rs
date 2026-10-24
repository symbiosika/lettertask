//! Google sign-in for Gmail accounts: authorization code flow with PKCE and
//! a loopback redirect, as Google prescribes for desktop apps ("installed
//! applications"). The token grants IMAP/SMTP access (scope
//! `https://mail.google.com/`); mail itself then runs over the IMAP/SMTP
//! code path with SASL XOAUTH2. Calendar and contacts use the Google Calendar
//! and People APIs (see `gapi`) with the same token.
//!
//! Register an OAuth client of type "Desktop app" in the Google Cloud
//! console. Google issues a client secret for it as well; for installed apps
//! it is not confidential and is sent along with the client id.

use std::time::Duration;

use serde::Deserialize;

use crate::auth::{post_token, post_token_scoped, random_state, LoopbackListener, Pkce, TokenSet};
use crate::error::{Error, Result};

pub const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
pub const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
pub const USERINFO_URL: &str = "https://openidconnect.googleapis.com/v1/userinfo";

/// Full mailbox access over IMAP/SMTP, calendars and contacts, plus who signed in.
pub const SCOPES: &[&str] = &[MAIL_SCOPE, crate::gapi::CALENDAR_SCOPE, crate::gapi::CONTACTS_SCOPE, "openid", "email", "profile"];
const MAIL_SCOPE: &str = "https://mail.google.com/";

pub const IMAP_HOST: &str = "imap.gmail.com";
pub const SMTP_HOST: &str = "smtp.gmail.com";

#[derive(Debug, Clone)]
pub struct GoogleConfig {
    pub client_id: String,
    /// Required by Google for desktop clients, not confidential.
    pub client_secret: String,
    pub auth_url: String,
    pub token_url: String,
    pub userinfo_url: String,
}

impl GoogleConfig {
    pub fn new(client_id: &str, client_secret: &str) -> Self {
        GoogleConfig {
            client_id: client_id.trim().to_string(),
            client_secret: client_secret.trim().to_string(),
            auth_url: AUTH_URL.into(),
            token_url: TOKEN_URL.into(),
            userinfo_url: USERINFO_URL.into(),
        }
    }

    fn check(&self) -> Result<()> {
        if self.client_id.is_empty() {
            return Err(Error::Auth(
                "Für Google ist keine Client-ID konfiguriert. Bitte unter Einstellungen › Konten › Google-Anmeldung eintragen.".into(),
            ));
        }
        Ok(())
    }

    fn with_secret<'a>(&'a self, form: &mut Vec<(&'a str, &'a str)>) {
        if !self.client_secret.is_empty() {
            form.push(("client_secret", self.client_secret.as_str()));
        }
    }
}

/// `login_hint` preselects the account (re-login of an existing account).
pub fn authorize_url(cfg: &GoogleConfig, redirect_uri: &str, state: &str, challenge: &str, login_hint: Option<&str>) -> String {
    let mut url = url::Url::parse(&cfg.auth_url).expect("valid Google auth URL");
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("client_id", &cfg.client_id)
            .append_pair("response_type", "code")
            .append_pair("redirect_uri", redirect_uri)
            .append_pair("scope", &SCOPES.join(" "))
            .append_pair("state", state)
            .append_pair("code_challenge", challenge)
            .append_pair("code_challenge_method", "S256")
            // A refresh token is only issued with offline access, and only
            // on the consent screen.
            .append_pair("access_type", "offline")
            .append_pair("prompt", "consent select_account");
        if let Some(h) = login_hint.filter(|h| !h.is_empty()) {
            q.append_pair("login_hint", h);
        }
    }
    url.to_string()
}

pub async fn exchange_code(
    http: &reqwest::Client,
    cfg: &GoogleConfig,
    code: &str,
    verifier: &str,
    redirect_uri: &str,
) -> Result<TokenSet> {
    let mut form = vec![
        ("client_id", cfg.client_id.as_str()),
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("code_verifier", verifier),
    ];
    cfg.with_secret(&mut form);
    let (tokens, scope) = post_token_scoped(http, &cfg.token_url, &form, None).await?;
    // Google lets the user untick single permissions on the consent page.
    if scope.is_some_and(|s| !s.split(' ').any(|x| x == MAIL_SCOPE)) {
        return Err(Error::Auth(
            "Google hat keinen Zugriff auf Gmail erteilt. Bitte erneut anmelden und auf der Zustimmungsseite den Zugriff auf Gmail (Häkchen) erlauben.".into(),
        ));
    }
    Ok(tokens)
}

pub async fn refresh(http: &reqwest::Client, cfg: &GoogleConfig, refresh_token: &str) -> Result<TokenSet> {
    cfg.check()?;
    let mut form = vec![
        ("client_id", cfg.client_id.as_str()),
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
    ];
    cfg.with_secret(&mut form);
    post_token(http, &cfg.token_url, &form, Some(refresh_token))
        .await
        .map_err(|e| match e {
            // Revoked, expired (7 days for apps in "testing" status) or the
            // password was changed: only a new sign-in helps.
            Error::Auth(m) if m.starts_with("invalid_grant") => Error::Auth(
                "Die Google-Anmeldung ist abgelaufen oder wurde widerrufen. Bitte das Konto erneut mit Google anmelden.".into(),
            ),
            e => e,
        })
}

#[derive(Debug, Clone, Deserialize)]
pub struct UserInfo {
    pub email: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub email_verified: Option<bool>,
}

/// Who signed in (OpenID Connect userinfo).
pub async fn user_info(http: &reqwest::Client, cfg: &GoogleConfig, access_token: &str) -> Result<UserInfo> {
    let resp = http.get(&cfg.userinfo_url).bearer_auth(access_token).send().await?;
    if !resp.status().is_success() {
        return Err(Error::Auth(format!("Google-Benutzerdaten nicht lesbar ({})", resp.status())));
    }
    let u: UserInfo = resp
        .json()
        .await
        .map_err(|e| Error::Auth(format!("Ungültige Google-Benutzerdaten: {e}")))?;
    if u.email.is_empty() || u.email_verified == Some(false) {
        return Err(Error::Auth("Google hat keine bestätigte E-Mail-Adresse geliefert.".into()));
    }
    Ok(u)
}

/// Full interactive login in the user's browser.
pub async fn interactive_login(
    http: &reqwest::Client,
    cfg: &GoogleConfig,
    login_hint: Option<&str>,
    open_browser: impl FnOnce(String) -> Result<()>,
    timeout: Duration,
) -> Result<TokenSet> {
    cfg.check()?;
    let mut listener = LoopbackListener::bind().await?;
    // Google wants the loopback IP literal for desktop clients.
    let redirect = format!("http://127.0.0.1:{}", listener.port);
    let pkce = Pkce::generate();
    let state = random_state();
    open_browser(authorize_url(cfg, &redirect, &state, &pkce.challenge, login_hint))?;
    let code = listener.wait_for_code(&state, timeout).await?;
    exchange_code(http, cfg, &code, &pkce.verifier, &redirect).await
}

/// SASL XOAUTH2 initial response (before base64), for IMAP and SMTP.
pub fn xoauth2(user: &str, access_token: &str) -> String {
    format!("user={user}\x01auth=Bearer {access_token}\x01\x01")
}

/// Gmail / Google Workspace servers (both use these hosts).
pub fn is_google_host(host: &str) -> bool {
    let h = host.trim().trim_end_matches('.').to_ascii_lowercase();
    h == IMAP_HOST || h == SMTP_HOST || h == "imap.googlemail.com" || h == "smtp.googlemail.com"
}

/// Addresses that are certainly Google accounts (Workspace domains are not
/// recognisable from the address).
pub fn is_gmail_address(email: &str) -> bool {
    let d = email.rsplit('@').next().unwrap_or("").trim().to_ascii_lowercase();
    d == "gmail.com" || d == "googlemail.com"
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::{get, post};
    use axum::{Form, Json, Router};
    use std::collections::HashMap;

    #[test]
    fn authorize_url_asks_for_offline_mail_access() {
        let cfg = GoogleConfig::new("cid.apps.googleusercontent.com", "sec");
        let u = authorize_url(&cfg, "http://127.0.0.1:5000", "st", "ch", Some("a@gmail.com"));
        let parsed = url::Url::parse(&u).unwrap();
        assert_eq!(parsed.host_str(), Some("accounts.google.com"));
        let q: HashMap<_, _> = parsed.query_pairs().into_owned().collect();
        assert_eq!(q["client_id"], "cid.apps.googleusercontent.com");
        assert_eq!(q["access_type"], "offline");
        assert_eq!(q["code_challenge_method"], "S256");
        assert_eq!(q["login_hint"], "a@gmail.com");
        assert!(q["scope"].split(' ').any(|s| s == "https://mail.google.com/"));
        // the secret never goes into the browser URL
        assert!(!u.contains("sec"));
    }

    #[test]
    fn xoauth2_format() {
        assert_eq!(xoauth2("a@b.c", "tok"), "user=a@b.c\x01auth=Bearer tok\x01\x01");
    }

    #[test]
    fn google_hosts_and_addresses() {
        assert!(is_google_host("imap.gmail.com"));
        assert!(is_google_host("SMTP.GMAIL.COM."));
        assert!(!is_google_host("imap.gmx.net"));
        assert!(is_gmail_address("x@GoogleMail.com"));
        assert!(!is_gmail_address("x@symbiosika.com"));
    }

    #[tokio::test]
    async fn missing_client_id_is_reported() {
        let cfg = GoogleConfig::new("", "");
        let e = interactive_login(&reqwest::Client::new(), &cfg, None, |_| Ok(()), Duration::from_secs(1))
            .await
            .unwrap_err();
        assert!(e.to_string().contains("Client-ID"), "{e}");
    }

    /// Login, refresh and userinfo against a minimal fake of Google's endpoints.
    #[tokio::test]
    async fn login_refresh_and_userinfo_against_fake() {
        async fn token(Form(f): Form<HashMap<String, String>>) -> (axum::http::StatusCode, Json<serde_json::Value>) {
            use axum::http::StatusCode;
            if f.get("client_secret").map(String::as_str) != Some("sec") {
                return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error": "invalid_client"})));
            }
            match f["grant_type"].as_str() {
                "authorization_code" if f["code"] == "CODE" && !f["code_verifier"].is_empty() => (
                    StatusCode::OK,
                    Json(serde_json::json!({"access_token": "at1", "refresh_token": "rt1", "expires_in": 3599,
                        "scope": "openid https://mail.google.com/ https://www.googleapis.com/auth/userinfo.email"})),
                ),
                "authorization_code" if f["code"] == "NOMAIL" => (
                    StatusCode::OK,
                    Json(serde_json::json!({"access_token": "at9", "refresh_token": "rt9", "expires_in": 3599,
                        "scope": "openid https://www.googleapis.com/auth/userinfo.email"})),
                ),
                "refresh_token" if f["refresh_token"] == "rt1" => {
                    (StatusCode::OK, Json(serde_json::json!({"access_token": "at2", "expires_in": 3599})))
                }
                _ => (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({"error": "invalid_grant", "error_description": "Bad Request"})),
                ),
            }
        }
        async fn userinfo(h: axum::http::HeaderMap) -> Json<serde_json::Value> {
            let ok = h.get("authorization").and_then(|v| v.to_str().ok()) == Some("Bearer at1");
            Json(if ok {
                serde_json::json!({"email": "test@gmail.com", "name": "Test", "email_verified": true})
            } else {
                serde_json::json!({})
            })
        }
        let app = Router::new().route("/token", post(token)).route("/userinfo", get(userinfo));
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });

        let mut cfg = GoogleConfig::new("cid", "sec");
        cfg.token_url = format!("{base}/token");
        cfg.userinfo_url = format!("{base}/userinfo");
        let http = reqwest::Client::new();
        // "Browser": follow the authorize URL straight back to the loopback.
        let t = interactive_login(
            &http,
            &cfg,
            None,
            |url| {
                let q: HashMap<String, String> = url::Url::parse(&url).unwrap().query_pairs().into_owned().collect();
                let back = format!("{}/?code=CODE&state={}", q["redirect_uri"], q["state"]);
                tokio::spawn(async move {
                    let _ = reqwest::get(back).await;
                });
                Ok(())
            },
            Duration::from_secs(10),
        )
        .await
        .unwrap();
        assert_eq!(t.access_token, "at1");
        assert_eq!(t.refresh_token, "rt1");
        let u = user_info(&http, &cfg, &t.access_token).await.unwrap();
        assert_eq!(u.email, "test@gmail.com");

        let r = refresh(&http, &cfg, "rt1").await.unwrap();
        assert_eq!(r.access_token, "at2");
        assert_eq!(r.refresh_token, "rt1", "refresh token is kept when not rotated");
        let e = exchange_code(&http, &cfg, "NOMAIL", "v", "http://127.0.0.1:1").await.unwrap_err();
        assert!(e.to_string().contains("Zugriff auf Gmail"), "{e}");
        let e = refresh(&http, &cfg, "revoked").await.unwrap_err();
        assert!(e.to_string().contains("erneut mit Google anmelden"), "{e}");
    }
}
