//! A stateful, in-memory fake of the Microsoft Graph v1.0 subset (mail,
//! calendar, categories, people, mailbox settings) and the Microsoft Entra
//! OAuth 2.0 authorization-code + PKCE endpoints.
//!
//! Used by the core integration tests, the app's `--demo` mode and the UI e2e
//! tests. JSON shapes follow the real service.

mod auth;
mod calendar;
mod contacts;
mod mail;
mod misc;
mod png;
mod seed;
pub mod smtp;
mod state;
mod util;

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::State as AxState;
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get, post, put};
use axum::Router;
use chrono::{DateTime, Utc};
use percent_encoding::percent_decode_str;

pub use state::{Fault, SharedState};
use util::{GErr, Req};

/// Options for [`spawn`].
#[derive(Clone, Debug)]
pub struct FakeOptions {
    /// Fill the mailboxes with German demo mails, events, categories and contacts.
    pub seed_demo: bool,
    /// Reference time for the seed data.
    pub now: DateTime<Utc>,
}

impl Default for FakeOptions {
    fn default() -> Self {
        FakeOptions { seed_demo: true, now: Utc::now() }
    }
}

/// A running fake server. Dropping it stops the server.
pub struct FakeServer {
    /// `http://127.0.0.1:PORT` – Graph lives at `{base_url}/v1.0`, OAuth at `{base_url}/{tenant}/oauth2/v2.0/...`.
    pub base_url: String,
    pub state: SharedState,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
}

impl FakeServer {
    /// `{base_url}/v1.0`
    pub fn graph_url(&self) -> String {
        format!("{}/v1.0", self.base_url)
    }
    /// Authority for a tenant, e.g. `{base_url}/organizations`.
    pub fn authority(&self, tenant: &str) -> String {
        format!("{}/{}", self.base_url, tenant)
    }
    /// A valid access token (skips the browser flow).
    pub fn issue_token(&self) -> String {
        self.state.issue_token()
    }
    /// Stops the server.
    pub fn shutdown(mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
    }
}

impl Drop for FakeServer {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
    }
}

/// Starts the fake on `127.0.0.1:0` (random free port).
pub async fn spawn(opts: FakeOptions) -> FakeServer {
    spawn_on(SocketAddr::from(([127, 0, 0, 1], 0)), opts).await.expect("bind fake graph server")
}

/// Starts the fake on a given address.
pub async fn spawn_on(addr: SocketAddr, opts: FakeOptions) -> std::io::Result<FakeServer> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let local = listener.local_addr()?;
    let base_url = format!("http://{}", local);
    let state = SharedState(Arc::new(Mutex::new(seed::build(&base_url, &opts))));
    let app = router(state.clone());
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = rx.await;
            })
            .await;
    });
    Ok(FakeServer { base_url, state, shutdown: Some(tx) })
}

fn router(state: SharedState) -> Router {
    Router::new()
        .route("/", get(|| async { "graph-fake: Microsoft Graph v1.0 test double. Graph at /v1.0" }))
        .route("/{tenant}/oauth2/v2.0/authorize", get(auth::authorize))
        .route("/{tenant}/oauth2/v2.0/token", post(auth::token))
        .route("/{tenant}/oauth2/v2.0/devicecode", post(auth::devicecode))
        .route("/upload/{sid}", put(mail::upload_chunk).delete(mail::upload_cancel))
        .route("/v1.0", any(graph))
        .route("/v1.0/{*rest}", any(graph))
        .fallback(|| async { GErr::new(StatusCode::NOT_FOUND, "UnknownError", "Resource not found").into_response() })
        .layer(axum::extract::DefaultBodyLimit::max(160 * 1024 * 1024))
        .with_state(state)
}

async fn graph(
    AxState(st): AxState<SharedState>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let raw_path = uri.path().strip_prefix("/v1.0").unwrap_or("").to_string();
    let segs: Vec<String> = raw_path
        .split('/')
        .filter(|s| !s.is_empty())
        .map(|s| percent_decode_str(s).decode_utf8_lossy().into_owned())
        .collect();
    let query: Vec<(String, String)> = uri
        .query()
        .map(|q| url::form_urlencoded::parse(q.as_bytes()).into_owned().collect())
        .unwrap_or_default();

    // auth + failure injection
    {
        let mut s = st.lock();
        s.request_count += 1;
        let token = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer ").or_else(|| v.strip_prefix("bearer ")))
            .map(str::trim)
            .unwrap_or("");
        if token.is_empty() {
            return GErr::new(StatusCode::UNAUTHORIZED, "InvalidAuthenticationToken", "Access token is empty.").into_response();
        }
        match s.auth.access.get(token) {
            Some(exp) if *exp > Utc::now() => {}
            Some(_) => {
                return GErr::new(StatusCode::UNAUTHORIZED, "InvalidAuthenticationToken", "Lifetime validation failed, the token is expired.").into_response()
            }
            None => {
                return GErr::new(StatusCode::UNAUTHORIZED, "InvalidAuthenticationToken", "IDX14100: JWT is not well formed.").into_response()
            }
        }
        let pos = s
            .faults
            .iter()
            .position(|f| f.path_contains.as_deref().is_none_or(|p| raw_path.contains(p)));
        if let Some(f) = pos.and_then(|p| s.faults.remove(p)) {
            let code = StatusCode::from_u16(f.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            let mut resp = match &f.body {
                Some(body) => (code, body.clone()).into_response(),
                None => {
                    let gcode = match f.status {
                        401 => "InvalidAuthenticationToken",
                        403 => "ErrorAccessDenied",
                        404 => "ErrorItemNotFound",
                        410 => "SyncStateNotFound",
                        429 => "TooManyRequests",
                        503 => "ServiceNotAvailable",
                        504 => "GatewayTimeout",
                        _ => "InternalServerError",
                    };
                    GErr::new(code, gcode, "Injected failure").into_response()
                }
            };
            if let Some(ct) = f.content_type.as_deref().and_then(|c| HeaderValue::from_str(c).ok()) {
                resp.headers_mut().insert(header::CONTENT_TYPE, ct);
            }
            if let Some(ra) = f.retry_after.as_deref().and_then(|r| HeaderValue::from_str(r).ok()) {
                resp.headers_mut().insert(header::RETRY_AFTER, ra);
            }
            return resp;
        }
    }

    let base_url = st.lock().base_url.clone();
    let req = Req { method, segs, raw_path, query, headers, body, base_url };
    match dispatch(&st, &req) {
        Ok(r) => r,
        Err(e) => e.into_response(),
    }
}

fn dispatch(st: &SharedState, req: &Req) -> util::GResult {
    let segs: Vec<&str> = req.segs.iter().map(String::as_str).collect();
    let (mb_key, rest): (&str, &[&str]) = match segs.as_slice() {
        ["me", rest @ ..] => ("me", rest),
        ["users", user, rest @ ..] => (user, rest),
        _ => return Err(GErr::new(StatusCode::BAD_REQUEST, "BadRequest", format!("Resource not found for the segment '{}'.", segs.first().unwrap_or(&"")))),
    };
    let mut s = st.lock();
    let Some(idx) = s.mailbox_idx(mb_key) else {
        return Err(GErr::new(
            StatusCode::NOT_FOUND,
            "ErrorInvalidUser",
            format!("The requested user '{mb_key}' is invalid."),
        ));
    };
    let first = rest.first().copied().unwrap_or("");
    match first {
        "" => {
            if req.method == Method::GET {
                util::ok(util::apply_select(s.mailboxes[idx].user_json(), req.q("select")))
            } else {
                Err(GErr::bad("Unsupported method"))
            }
        }
        "mailFolders" | "messages" | "sendMail" => mail::handle(&mut s, idx, rest, req),
        "calendars" | "calendar" | "events" | "calendarView" => calendar::handle(&mut s.mailboxes[idx], rest, req),
        "people" | "outlook" | "mailboxSettings" => misc::handle(&mut s.mailboxes[idx], rest, req),
        "contacts" => contacts::handle(&mut s.mailboxes[idx], rest, req),
        other => Err(GErr::new(
            StatusCode::BAD_REQUEST,
            "BadRequest",
            format!("Resource not found for the segment '{other}'."),
        )),
    }
}
