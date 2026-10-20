//! Development/test server: serves `ui/` and exposes the service API over HTTP
//! (`POST /api/call/<cmd>`, events as SSE on `/api/events`).
//!
//! `cargo run -p devserver -- --demo --port 8080` runs against the built-in
//! fake Microsoft Graph with demo data; without `--demo` it talks to the real
//! Microsoft 365 (login URL is printed to the console).

use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use async_trait::async_trait;
use axum::extract::{Path as AxPath, Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::Stream;
use mailcore::tokens::{FileTokenStore, MemoryTokenStore, TokenStore};
use mailcore::{Host, Service, ServiceOptions};
use serde_json::{json, Value};
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt as _;
use tower_http::services::ServeDir;

struct HttpHost {
    tx: broadcast::Sender<Value>,
    downloads: PathBuf,
}

#[async_trait]
impl Host for HttpHost {
    fn emit(&self, event: &str, payload: Value) {
        let _ = self.tx.send(json!({"event": event, "payload": payload}));
    }
    fn open_url(&self, url: &str) -> mailcore::Result<()> {
        println!("Bitte im Browser öffnen: {url}");
        Ok(())
    }
    fn open_path(&self, path: &Path) -> mailcore::Result<()> {
        println!("Datei gespeichert: {}", path.display());
        Ok(())
    }
    async fn save_dialog(&self, suggested_name: &str) -> Option<PathBuf> {
        let _ = std::fs::create_dir_all(&self.downloads);
        Some(self.downloads.join(suggested_name))
    }
    fn notify(&self, title: &str, body: &str) {
        println!("Benachrichtigung: {title} – {body}");
    }
    fn open_window(&self, kind: &str, query: &str) -> mailcore::Result<()> {
        // Browser clients open windows themselves; forward as event.
        self.emit("open-window", json!({"kind": kind, "query": query}));
        Ok(())
    }
}

#[derive(Clone)]
struct AppState {
    svc: Arc<Service>,
    tx: broadcast::Sender<Value>,
}

async fn call(State(st): State<AppState>, AxPath(cmd): AxPath<String>, body: Option<Json<Value>>) -> Response {
    let args = body.map(|Json(v)| v).unwrap_or(Value::Object(Default::default()));
    match st.svc.dispatch(&cmd, args).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => {
            let status = match &e {
                mailcore::Error::Invalid(_) => StatusCode::BAD_REQUEST,
                mailcore::Error::NotFound(_) => StatusCode::NOT_FOUND,
                mailcore::Error::Auth(_) => StatusCode::UNAUTHORIZED,
                _ => StatusCode::BAD_GATEWAY,
            };
            (status, Json(json!({"error": e.to_string()}))).into_response()
        }
    }
}

async fn events(State(st): State<AppState>) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let stream = BroadcastStream::new(st.tx.subscribe())
        .filter_map(|m| m.ok())
        .map(|v| Ok(Event::default().data(v.to_string())));
    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// The app windows' Content-Security-Policy, taken from the Tauri config so the
/// browser (and the e2e tests) run the UI under exactly the same policy.
fn app_csp() -> String {
    let conf: Value = serde_json::from_str(include_str!("../../../src-tauri/tauri.conf.json")).expect("tauri.conf.json");
    conf["app"]["security"]["csp"].as_str().unwrap_or("default-src 'self'").to_string()
}

/// Only requests addressed to this loopback server are served: a foreign Host
/// header means DNS rebinding, a foreign Origin a cross-site request (CSRF).
fn request_allowed(host: Option<&str>, origin: Option<&str>, port: u16) -> bool {
    let local = [format!("127.0.0.1:{port}"), format!("localhost:{port}")];
    let Some(host) = host else { return false };
    if !local.iter().any(|l| l.eq_ignore_ascii_case(host)) {
        return false;
    }
    match origin {
        None => true,
        Some(o) => local.iter().any(|l| o.eq_ignore_ascii_case(&format!("http://{l}"))),
    }
}

#[derive(Clone)]
struct Guard {
    port: u16,
    csp: HeaderValue,
}

async fn guard(State(g): State<Guard>, req: Request, next: Next) -> Response {
    let allowed = {
        let hdr = |n: header::HeaderName| req.headers().get(n).and_then(|v| v.to_str().ok());
        request_allowed(hdr(header::HOST), hdr(header::ORIGIN), g.port)
    };
    if !allowed {
        return (StatusCode::FORBIDDEN, "Zugriff verweigert").into_response();
    }
    let mut res = next.run(req).await;
    res.headers_mut().insert(header::CONTENT_SECURITY_POLICY, g.csp);
    res.headers_mut().insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    res
}

struct Args {
    demo: bool,
    port: u16,
    data_dir: Option<PathBuf>,
    ui_dir: PathBuf,
}

fn parse_args() -> anyhow::Result<Args> {
    let mut a = Args {
        demo: false,
        port: 8080,
        data_dir: None,
        ui_dir: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../ui"),
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--demo" => a.demo = true,
            "--port" => a.port = it.next().context("--port braucht einen Wert")?.parse()?,
            "--data-dir" => a.data_dir = Some(it.next().context("--data-dir braucht einen Wert")?.into()),
            "--ui-dir" => a.ui_dir = it.next().context("--ui-dir braucht einen Wert")?.into(),
            "-h" | "--help" => {
                println!("devserver [--demo] [--port 8080] [--data-dir DIR] [--ui-dir DIR]");
                std::process::exit(0);
            }
            other => anyhow::bail!("Unbekanntes Argument: {other}"),
        }
    }
    Ok(a)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    let args = parse_args()?;
    let (tx, _) = broadcast::channel(256);

    let (data_dir, graph_base, authority, tokens, _fake): (PathBuf, String, String, Arc<dyn TokenStore>, _) = if args.demo {
        let fake = graph_fake::spawn(graph_fake::FakeOptions {
            seed_demo: true,
            now: chrono::Utc::now(),
        })
        .await;
        // Fresh state every start so tests are reproducible.
        let dir = args.data_dir.clone().unwrap_or_else(|| {
            std::env::temp_dir().join(format!("lettertask-demo-{}", std::process::id()))
        });
        let _ = std::fs::remove_dir_all(&dir);
        (
            dir,
            format!("{}/v1.0", fake.base_url),
            fake.base_url.clone(),
            Arc::new(MemoryTokenStore::default()),
            Some(fake),
        )
    } else {
        let dir = args
            .data_dir
            .clone()
            .unwrap_or_else(|| std::env::temp_dir().join("lettertask-dev"));
        let tokens: Arc<dyn TokenStore> = Arc::new(FileTokenStore::new(dir.join("tokens.json")));
        (
            dir,
            mailcore::graph::DEFAULT_GRAPH_BASE.to_string(),
            mailcore::auth::DEFAULT_AUTHORITY.to_string(),
            tokens,
            None,
        )
    };

    let host = Arc::new(HttpHost {
        tx: tx.clone(),
        downloads: data_dir.join("downloads"),
    });
    let svc = Service::new(
        ServiceOptions {
            data_dir,
            graph_base,
            authority,
            demo: args.demo,
            token_store: tokens,
        },
        host,
    )?;
    if args.demo {
        svc.ensure_demo_account().await?;
    }
    svc.start_background_sync();

    let state = AppState { svc, tx };
    let g = Guard {
        port: args.port,
        csp: HeaderValue::from_str(&app_csp())?,
    };
    let app = Router::new()
        .route("/api/call/{cmd}", post(call))
        .route("/api/events", get(events))
        .fallback_service(ServeDir::new(&args.ui_dir))
        .layer(axum::middleware::from_fn_with_state(g, guard))
        .with_state(state);

    let addr = SocketAddr::from(([127, 0, 0, 1], args.port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("lettertask Dev-Server läuft auf http://{addr}/ {}", if args.demo { "(Demo-Modus)" } else { "" });
    axum::serve(listener, app).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_loopback_host_and_origin_are_served() {
        assert!(request_allowed(Some("127.0.0.1:8080"), None, 8080));
        assert!(request_allowed(Some("localhost:8080"), Some("http://localhost:8080"), 8080));
        // DNS rebinding: evil.example resolves to 127.0.0.1
        assert!(!request_allowed(Some("evil.example:8080"), None, 8080));
        assert!(!request_allowed(None, None, 8080));
        assert!(!request_allowed(Some("127.0.0.1:9999"), None, 8080));
        // cross-site POST from a web page
        assert!(!request_allowed(Some("127.0.0.1:8080"), Some("https://evil.example"), 8080));
        assert!(!request_allowed(Some("127.0.0.1:8080"), Some("null"), 8080));
    }

    #[test]
    fn csp_comes_from_tauri_config() {
        let csp = app_csp();
        assert!(csp.contains("script-src 'self'"), "{csp}");
        assert!(csp.contains("object-src 'none'"), "{csp}");
        assert!(!csp.contains("unsafe-eval"), "{csp}");
    }
}
