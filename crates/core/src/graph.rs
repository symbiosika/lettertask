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

    async fn access_token(&self, force_refresh: bool) -> Result<String> {
        let mut t = self.tokens.lock().await;
        if force_refresh || t.needs_refresh() {
            let fresh = auth::refresh(&self.http, &self.oauth, &t.refresh_token).await?;
            if fresh.refresh_token != t.refresh_token {
                self.store.save(&self.account_id, &fresh.refresh_token)?;
            }
            *t = fresh;
        }
        Ok(t.access_token.clone())
    }

    fn url(&self, path: &str) -> String {
        if path.starts_with("http://") || path.starts_with("https://") {
            path.to_string()
        } else {
            format!("{}{}", self.base, path)
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
        let url = self.url(path);
        let mut refreshed = false;
        let mut attempt = 0;
        loop {
            attempt += 1;
            let token = self.access_token(false).await?;
            let mut req = self.http.request(method.clone(), &url).bearer_auth(token);
            for (k, v) in headers {
                req = req.header(*k, v);
            }
            if let Some(b) = body {
                req = req.json(b);
            } else if method == Method::POST {
                req = req.header("Content-Length", "0");
            }
            let resp = req.send().await?;
            let status = resp.status();
            if status == StatusCode::UNAUTHORIZED && !refreshed {
                refreshed = true;
                self.access_token(true).await?;
                continue;
            }
            if (status == StatusCode::TOO_MANY_REQUESTS
                || status == StatusCode::SERVICE_UNAVAILABLE
                || status == StatusCode::GATEWAY_TIMEOUT)
                && attempt < 4
            {
                let wait = resp
                    .headers()
                    .get("Retry-After")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(attempt as u64 * 2)
                    .min(30);
                tokio::time::sleep(Duration::from_secs(wait)).await;
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
        let text = resp.text().await?;
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        Ok(serde_json::from_str(&text)?)
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
        while let Some(p) = next.take() {
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

async fn error_from_response(resp: reqwest::Response) -> Error {
    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    let code = v["error"]["code"].as_str().unwrap_or("").to_string();
    let message = v["error"]["message"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| text.chars().take(300).collect());
    if status == 401 {
        return Error::Auth(format!("{code} {message}").trim().to_string());
    }
    Error::Graph {
        status,
        code,
        message,
    }
}

/// Percent-encodes a Graph id for use in a URL path segment.
pub fn seg(id: &str) -> String {
    urlencoding::encode(id).into_owned()
}
