//! Thin authenticated HTTP client for Microsoft Graph.

use std::sync::Arc;
use std::time::Duration;

use reqwest::{Method, StatusCode};
use serde_json::Value;
use tokio::sync::Mutex;

use crate::auth::{self, OAuthConfig, TokenSet};
use crate::error::{Error, Result};
use crate::tokens::TokenStore;

pub const DEFAULT_GRAPH_BASE: &str = "https://graph.microsoft.com/v1.0";

/// Authenticated Graph client for one signed-in account.
pub struct GraphClient {
    http: reqwest::Client,
    base: String,
    account_id: String,
    oauth: OAuthConfig,
    tokens: Mutex<TokenSet>,
    store: Arc<dyn TokenStore>,
}

impl GraphClient {
    pub fn new(
        http: reqwest::Client,
        base: impl Into<String>,
        account_id: impl Into<String>,
        oauth: OAuthConfig,
        tokens: TokenSet,
        store: Arc<dyn TokenStore>,
    ) -> Self {
        GraphClient {
            http,
            base: base.into().trim_end_matches('/').to_string(),
            account_id: account_id.into(),
            oauth,
            tokens: Mutex::new(tokens),
            store,
        }
    }

    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    pub fn set_account_id(&mut self, id: impl Into<String>) {
        self.account_id = id.into();
    }

    /// Current access token, refreshed when about to expire. With `stale`
    /// set (after a 401), a refresh happens only if no other request has
    /// already replaced that token meanwhile – parallel requests failing at
    /// once must not each burn a refresh.
    async fn access_token(&self, stale: Option<&str>) -> Result<String> {
        let mut t = self.tokens.lock().await;
        let force = stale.is_some_and(|s| s == t.access_token);
        if force || t.needs_refresh() {
            let fresh = auth::refresh(&self.http, &self.oauth, &t.refresh_token)
                .await
                .map_err(|e| match e {
                    Error::Auth(detail) => Error::Auth(format!(
                        "Die Anmeldung ist abgelaufen oder wurde widerrufen. Bitte das Konto erneut anmelden ({detail})."
                    )),
                    other => other,
                })?;
            if fresh.refresh_token != t.refresh_token {
                self.store.save(&self.account_id, &fresh.refresh_token)?;
            }
            *t = fresh;
        }
        Ok(t.access_token.clone())
    }

    fn origin(url: &str) -> &str {
        let after_scheme = url.find("://").map_or(0, |i| i + 3);
        let end = url[after_scheme..].find('/').map_or(url.len(), |i| after_scheme + i);
        &url[..end]
    }

    fn url(&self, path: &str) -> Result<String> {
        if path.starts_with("http://") || path.starts_with("https://") {
            // Paging/delta links come from Graph. Never send the token elsewhere.
            if !Self::origin(path).eq_ignore_ascii_case(Self::origin(&self.base)) {
                return Err(Error::Other(format!(
                    "Unerwartete Adresse in der Serverantwort: {}",
                    Self::origin(path)
                )));
            }
            Ok(path.to_string())
        } else {
            Ok(format!("{}{}", self.base, path))
        }
    }

    /// Sends a request with auth, retrying throttled/transient failures and
    /// refreshing the token once on 401.
    pub async fn send(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
        headers: &[(&str, String)],
    ) -> Result<reqwest::Response> {
        let url = self.url(path)?;
        // Only requests that are safe to repeat are retried after a network
        // error; a POST may already have been executed (e.g. sendMail).
        let idempotent = matches!(method, Method::GET | Method::HEAD | Method::PUT | Method::DELETE);
        let mut refreshed = false;
        let mut net_retried = false;
        let mut attempt: u32 = 0;
        loop {
            attempt += 1;
            let token = self.access_token(None).await?;
            let mut req = self.http.request(method.clone(), &url).bearer_auth(&token);
            for (k, v) in headers {
                req = req.header(*k, v);
            }
            if let Some(b) = body {
                req = req.json(b);
            } else if method == Method::POST {
                req = req.header("Content-Length", "0");
            }
            let resp = match req.send().await {
                Ok(r) => r,
                Err(e) if idempotent && !net_retried && (e.is_connect() || (e.is_request() && !e.is_timeout())) => {
                    net_retried = true;
                    tracing::debug!("Netzwerkfehler, neuer Versuch: {e}");
                    tokio::time::sleep(Duration::from_millis(250)).await;
                    continue;
                }
                Err(e) => return Err(e.into()),
            };
            let status = resp.status();
            if status == StatusCode::UNAUTHORIZED {
                if !refreshed {
                    refreshed = true;
                    self.access_token(Some(&token)).await?;
                    continue;
                }
                let err = error_from_response(resp).await;
                return Err(Error::Auth(format!(
                    "Der Server lehnt die Anmeldung ab. Bitte das Konto erneut anmelden ({}).",
                    err.to_string().trim_start_matches("Anmeldung erforderlich: ")
                )));
            }
            let transient = status == StatusCode::TOO_MANY_REQUESTS
                || status == StatusCode::SERVICE_UNAVAILABLE
                || status == StatusCode::GATEWAY_TIMEOUT
                || (status == StatusCode::BAD_GATEWAY && idempotent);
            if transient && attempt < MAX_ATTEMPTS {
                let wait = retry_after(resp.headers()).unwrap_or_else(|| Duration::from_secs(1 << (attempt - 1)));
                tokio::time::sleep(wait.min(MAX_RETRY_WAIT)).await;
                continue;
            }
            if !status.is_success() {
                return Err(error_from_response(resp).await);
            }
            return Ok(resp);
        }
    }

    pub async fn request_json(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
        headers: &[(&str, String)],
    ) -> Result<Value> {
        let resp = self.send(method, path, body, headers).await?;
        let status = resp.status().as_u16();
        let text = resp.text().await?;
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        // A success status with a broken body (proxy page, cut-off
        // transfer) is a server problem, not invalid user input.
        serde_json::from_str(&text).map_err(|e| Error::Graph {
            status,
            code: "InvalidResponse".into(),
            message: format!("Ungültige Antwort vom Server: {e}"),
        })
    }

    pub async fn get(&self, path: &str) -> Result<Value> {
        self.request_json(Method::GET, path, None, &[]).await
    }

    pub async fn get_h(&self, path: &str, headers: &[(&str, String)]) -> Result<Value> {
        self.request_json(Method::GET, path, None, headers).await
    }

    pub async fn post(&self, path: &str, body: &Value) -> Result<Value> {
        self.request_json(Method::POST, path, Some(body), &[]).await
    }

    pub async fn post_empty(&self, path: &str) -> Result<Value> {
        self.request_json(Method::POST, path, None, &[]).await
    }

    pub async fn patch(&self, path: &str, body: &Value) -> Result<Value> {
        self.request_json(Method::PATCH, path, Some(body), &[]).await
    }

    pub async fn delete(&self, path: &str) -> Result<()> {
        self.send(Method::DELETE, path, None, &[]).await?;
        Ok(())
    }

    pub async fn get_bytes(&self, path: &str) -> Result<Vec<u8>> {
        let resp = self.send(Method::GET, path, None, &[]).await?;
        Ok(resp.bytes().await?.to_vec())
    }

    /// Follows `@odata.nextLink` until `max` items were collected.
    pub async fn get_all(&self, path: &str, headers: &[(&str, String)], max: usize) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        let mut next = Some(path.to_string());
        let mut seen = std::collections::HashSet::new();
        while let Some(p) = next.take() {
            if !seen.insert(self.url(&p)?) {
                // A server returning the same link again would loop forever.
                tracing::warn!("@odata.nextLink wiederholt sich – Abbruch nach {} Einträgen", out.len());
                break;
            }
            let page = self.get_h(&p, headers).await?;
            if let Some(items) = page.get("value").and_then(Value::as_array) {
                out.extend(items.iter().cloned());
            }
            if out.len() >= max {
                out.truncate(max);
                break;
            }
            next = page
                .get("@odata.nextLink")
                .and_then(Value::as_str)
                .map(str::to_string);
        }
        Ok(out)
    }

    /// PUT a chunk to an attachment upload session URL. Those URLs are
    /// pre-authorized and must NOT carry the bearer token.
    pub async fn put_upload_chunk(&self, url: &str, chunk: Vec<u8>, start: usize, total: usize) -> Result<()> {
        let end = start + chunk.len() - 1;
        let resp = self
            .http
            .put(url)
            .header("Content-Range", format!("bytes {start}-{end}/{total}"))
            .header("Content-Type", "application/octet-stream")
            .body(chunk)
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(error_from_response(resp).await);
        }
        Ok(())
    }
}

const MAX_ATTEMPTS: u32 = 4;
const MAX_RETRY_WAIT: Duration = Duration::from_secs(30);

/// `Retry-After` as seconds or HTTP date.
fn retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let v = headers.get(reqwest::header::RETRY_AFTER)?.to_str().ok()?.trim();
    if let Ok(secs) = v.parse::<f64>() {
        return (secs.is_finite() && secs >= 0.0).then(|| Duration::from_secs_f64(secs.min(3600.0)));
    }
    let at = chrono::DateTime::parse_from_rfc2822(v).ok()?;
    let secs = (at.with_timezone(&chrono::Utc) - chrono::Utc::now()).num_milliseconds().max(0);
    Some(Duration::from_millis(secs as u64))
}

/// Short plain-text excerpt of a non-JSON error body (e.g. an HTML proxy page).
fn text_excerpt(text: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for ch in text.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    let collapsed = out.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed.chars().take(200).collect()
}

async fn error_from_response(resp: reqwest::Response) -> Error {
    let status = resp.status().as_u16();
    let reason = resp.status().canonical_reason().unwrap_or("").to_string();
    let text = resp.text().await.unwrap_or_default();
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    let code = v["error"]["code"].as_str().unwrap_or("").to_string();
    let message = v["error"]["message"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| {
            let excerpt = text_excerpt(&text);
            if excerpt.is_empty() {
                reason.clone()
            } else {
                excerpt
            }
        });
    if status == 401 {
        return Error::Auth(format!("{code} {message}").trim().to_string());
    }
    Error::Graph {
        status,
        code,
        message,
    }
}

/// Percent-encodes a Graph id for use in a URL path segment. `.` and `..`
/// are encoded too: left as they are, URL normalisation would turn
/// `mailFolders/../messages` into a request on a different resource.
pub fn seg(id: &str) -> String {
    match id {
        "." => "%2E".into(),
        ".." => "%2E%2E".into(),
        _ => urlencoding::encode(id).into_owned(),
    }
}

#[cfg(test)]
mod seg_tests {
    #[test]
    fn dot_segments_are_encoded() {
        assert_eq!(super::seg("."), "%2E");
        assert_eq!(super::seg(".."), "%2E%2E");
        assert_eq!(super::seg("a/b"), "a%2Fb");
        assert_eq!(super::seg("AAMk=="), "AAMk%3D%3D");
    }
}
