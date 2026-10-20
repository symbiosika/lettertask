//! Graph client robustness: token refresh, throttling, transient errors,
//! paging, odd responses – against small scripted servers and the fake.


use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, post};
use axum::Router;
use crate::common::*;
use mailcore::auth::{OAuthConfig, TokenSet};
use mailcore::graph::GraphClient;
use mailcore::model::*;
use mailcore::tokens::MemoryTokenStore;
use mailcore::Error;
use serde_json::{json, Value};

// ------------------------------------------------------------ scripted server

type Handler = Arc<dyn Fn(&str, &HeaderMap, usize) -> Response + Send + Sync>;

#[derive(Clone)]
struct Mock {
    handler: Handler,
    hits: Arc<AtomicUsize>,
    tokens: Arc<AtomicUsize>,
    /// Answer of the token endpoint: Ok(access token) or Err(error code).
    token_answer: Arc<Mutex<Result<String, String>>>,
}

async fn graph_route(State(m): State<Mock>, req: Request) -> Response {
    let n = m.hits.fetch_add(1, Ordering::SeqCst);
    let path = req.uri().path_and_query().map(|p| p.as_str().to_string()).unwrap_or_default();
    (m.handler)(&path, req.headers(), n)
}

async fn token_route(State(m): State<Mock>) -> Response {
    m.tokens.fetch_add(1, Ordering::SeqCst);
    // Slow enough that parallel requests pile up on the refresh.
    tokio::time::sleep(Duration::from_millis(50)).await;
    match m.token_answer.lock().unwrap().clone() {
        Ok(at) => axum::Json(json!({"access_token": at, "refresh_token": "rt-2", "expires_in": 3600, "token_type": "Bearer"})).into_response(),
        Err(code) => (
            StatusCode::BAD_REQUEST,
            axum::Json(json!({"error": code, "error_description": "AADSTS70000: Provided grant is invalid or malformed.\r\nTrace ID: x"})),
        )
            .into_response(),
    }
}

struct Server {
    base: String,
    mock: Mock,
}

async fn serve(handler: impl Fn(&str, &HeaderMap, usize) -> Response + Send + Sync + 'static) -> Server {
    let mock = Mock {
        handler: Arc::new(handler),
        hits: Arc::default(),
        tokens: Arc::default(),
        token_answer: Arc::new(Mutex::new(Ok("at-new".into()))),
    };
    let app = Router::new()
        .route("/t/oauth2/v2.0/token", post(token_route))
        .route("/v1.0/{*rest}", any(graph_route))
        .with_state(mock.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Server { base, mock }
}

impl Server {
    fn client(&self, access_token: &str) -> GraphClient {
        let http = reqwest::Client::builder().timeout(Duration::from_secs(20)).build().unwrap();
        GraphClient::new(
            http,
            format!("{}/v1.0", self.base),
            "acc",
            OAuthConfig { authority: self.base.clone(), tenant: "t".into(), client_id: "cid".into() },
            TokenSet {
                access_token: access_token.into(),
                refresh_token: "rt-1".into(),
                expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
            },
            Arc::new(MemoryTokenStore::default()),
        )
    }
    fn hits(&self) -> usize {
        self.mock.hits.load(Ordering::SeqCst)
    }
    fn token_requests(&self) -> usize {
        self.mock.tokens.load(Ordering::SeqCst)
    }
}

fn bearer(h: &HeaderMap) -> String {
    h.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("").to_string()
}

fn json_resp(status: u16, v: Value) -> Response {
    (StatusCode::from_u16(status).unwrap(), axum::Json(v)).into_response()
}

fn graph_error(status: u16, code: &str) -> Response {
    json_resp(status, json!({"error": {"code": code, "message": format!("{code} message")}}))
}

fn with_retry_after(mut r: Response, v: &'static str) -> Response {
    r.headers_mut().insert("Retry-After", v.parse().unwrap());
    r
}

// ------------------------------------------------------------ token handling

#[tokio::test]
async fn unauthorized_refreshes_once_and_retries() {
    let srv = serve(|_, h, _| {
        if bearer(h) == "Bearer at-new" {
            json_resp(200, json!({"ok": true}))
        } else {
            graph_error(401, "InvalidAuthenticationToken")
        }
    })
    .await;
    let c = srv.client("at-old");
    assert_eq!(c.get("/me").await.unwrap()["ok"], true);
    assert_eq!((srv.hits(), srv.token_requests()), (2, 1));
    // The new token is kept.
    c.get("/me").await.unwrap();
    assert_eq!((srv.hits(), srv.token_requests()), (3, 1));
}

#[tokio::test]
async fn still_unauthorized_after_refresh_asks_for_login() {
    let srv = serve(|_, _, _| graph_error(401, "InvalidAuthenticationToken")).await;
    let c = srv.client("at-old");
    let e = c.get("/me").await.unwrap_err();
    assert!(matches!(e, Error::Auth(_)), "{e:?}");
    assert!(e.to_string().contains("erneut anmelden"), "{e}");
    assert_eq!(srv.hits(), 2, "exactly one retry");
    assert_eq!(srv.token_requests(), 1);
}

#[tokio::test]
async fn failed_refresh_is_a_clear_relogin_error() {
    let srv = serve(|_, _, _| graph_error(401, "InvalidAuthenticationToken")).await;
    *srv.mock.token_answer.lock().unwrap() = Err("invalid_grant".into());
    let c = srv.client("at-old");
    let e = c.get("/me").await.unwrap_err();
    let msg = e.to_string();
    assert!(matches!(e, Error::Auth(_)), "{e:?}");
    assert!(msg.contains("Bitte das Konto erneut anmelden") && msg.contains("invalid_grant"), "{msg}");
    assert!(!msg.contains("Trace ID"), "only the first line of the description: {msg}");
}

#[tokio::test]
async fn parallel_401s_share_one_refresh() {
    let srv = serve(|_, h, _| {
        if bearer(h) == "Bearer at-new" {
            json_resp(200, json!({"ok": true}))
        } else {
            graph_error(401, "InvalidAuthenticationToken")
        }
    })
    .await;
    let c = Arc::new(srv.client("at-old"));
    let tasks: Vec<_> = (0..8)
        .map(|_| {
            let c = c.clone();
            tokio::spawn(async move { c.get("/me/messages").await })
        })
        .collect();
    for t in tasks {
        t.await.unwrap().unwrap();
    }
    assert_eq!(srv.token_requests(), 1, "one refresh for all parallel requests");
}

// ------------------------------------------------------------ throttling / transient

#[tokio::test]
async fn throttling_honours_retry_after() {
    let srv = serve(|_, _, n| {
        if n == 0 {
            with_retry_after(graph_error(429, "TooManyRequests"), "1")
        } else {
            json_resp(200, json!({"value": []}))
        }
    })
    .await;
    let t = Instant::now();
    srv.client("at").get("/me/messages").await.unwrap();
    assert!(t.elapsed() >= Duration::from_millis(950), "waited {:?}", t.elapsed());
    assert_eq!(srv.hits(), 2);
}

#[tokio::test]
async fn retry_after_as_http_date() {
    // A date in the past means "retry now".
    let srv = serve(|_, _, n| {
        if n < 2 {
            with_retry_after(graph_error(503, "ServiceNotAvailable"), "Wed, 21 Oct 2015 07:28:00 GMT")
        } else {
            json_resp(200, json!({"ok": 1}))
        }
    })
    .await;
    let t = Instant::now();
    srv.client("at").get("/x").await.unwrap();
    assert!(t.elapsed() < Duration::from_secs(1), "{:?}", t.elapsed());
    assert_eq!(srv.hits(), 3);
}

#[tokio::test]
async fn persistent_throttling_gives_up() {
    let srv = serve(|_, _, _| with_retry_after(graph_error(429, "TooManyRequests"), "0")).await;
    let e = srv.client("at").get("/x").await.unwrap_err();
    assert!(matches!(e, Error::Graph { status: 429, .. }), "{e:?}");
    assert_eq!(srv.hits(), 4, "bounded number of attempts");
}

#[tokio::test]
async fn bad_gateway_is_retried_only_for_safe_methods() {
    let srv = serve(|_, _, n| {
        if n == 0 {
            with_retry_after((StatusCode::BAD_GATEWAY, "<html><body><h1>502 Bad Gateway</h1></body></html>").into_response(), "0")
        } else {
            json_resp(200, json!({"ok": 1}))
        }
    })
    .await;
    srv.client("at").get("/x").await.unwrap();
    assert_eq!(srv.hits(), 2);

    // A POST (sendMail!) may already have happened: no blind repeat.
    let srv = serve(|_, _, _| with_retry_after((StatusCode::BAD_GATEWAY, "<html><h1>502 Bad Gateway</h1></html>").into_response(), "0")).await;
    let e = srv.client("at").post("/me/sendMail", &json!({})).await.unwrap_err();
    assert_eq!(srv.hits(), 1);
    match e {
        Error::Graph { status, message, .. } => {
            assert_eq!(status, 502);
            assert_eq!(message, "502 Bad Gateway", "HTML is reduced to its text");
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn server_errors_with_odd_bodies() {
    let srv = serve(|path, _, _| match path {
        p if p.starts_with("/v1.0/html") => (StatusCode::INTERNAL_SERVER_ERROR, "<!DOCTYPE html><html><head><title>Fehler</title></head><body>Interner Fehler</body></html>").into_response(),
        p if p.starts_with("/v1.0/empty") => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        p if p.starts_with("/v1.0/long") => (StatusCode::INTERNAL_SERVER_ERROR, "x".repeat(10_000)).into_response(),
        p if p.starts_with("/v1.0/garbage") => (StatusCode::OK, "{\"value\": [1, 2").into_response(),
        p if p.starts_with("/v1.0/nocontent") => StatusCode::NO_CONTENT.into_response(),
        _ => graph_error(404, "ErrorItemNotFound"),
    })
    .await;
    let c = srv.client("at");
    match c.get("/html").await.unwrap_err() {
        Error::Graph { status: 500, message, .. } => assert_eq!(message, "Fehler Interner Fehler"),
        other => panic!("{other:?}"),
    }
    match c.get("/empty").await.unwrap_err() {
        Error::Graph { status: 500, message, .. } => assert_eq!(message, "Internal Server Error"),
        other => panic!("{other:?}"),
    }
    match c.get("/long").await.unwrap_err() {
        Error::Graph { message, .. } => assert!(message.chars().count() <= 200),
        other => panic!("{other:?}"),
    }
    // A cut-off success body is a server problem, not "invalid input".
    match c.get("/garbage").await.unwrap_err() {
        Error::Graph { status: 200, code, message } => {
            assert_eq!(code, "InvalidResponse");
            assert!(message.contains("Ungültige Antwort"), "{message}");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(c.get("/nocontent").await.unwrap(), Value::Null);
    let e = c.get("/missing").await.unwrap_err();
    assert!(e.is_not_found());
}

#[tokio::test]
async fn connection_refused_is_a_network_error() {
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let srv = Server { base: format!("http://127.0.0.1:{port}"), mock: serve(|_, _, _| StatusCode::OK.into_response()).await.mock };
    let e = srv.client("at").get("/me").await.unwrap_err();
    assert!(e.is_network(), "{e:?}");
    assert!(e.to_string().starts_with("Keine Verbindung"), "{e}");
}

// ------------------------------------------------------------ paging

#[tokio::test]
async fn next_link_paging_and_limits() {
    // nextLinks are absolute URLs on the Graph host.
    let addr = Arc::new(Mutex::new(String::new()));
    let a2 = addr.clone();
    let srv = serve(move |path, _, _| {
        let page: usize = path.split("page=").nth(1).and_then(|p| p.parse().ok()).unwrap_or(0);
        let items: Vec<Value> = (0..10).map(|i| json!({"id": format!("{page}-{i}")})).collect();
        let mut v = json!({"value": items});
        if page < 4 {
            v["@odata.nextLink"] = json!(format!("{}/v1.0/items?page={}", a2.lock().unwrap(), page + 1));
        }
        json_resp(200, v)
    })
    .await;
    *addr.lock().unwrap() = srv.base.clone();
    let c = srv.client("at");
    let all = c.get_all("/items?page=0", &[], 1000).await.unwrap();
    assert_eq!(all.len(), 50);
    assert_eq!(all[49]["id"], "4-9");
    let limited = c.get_all("/items?page=0", &[], 15).await.unwrap();
    assert_eq!(limited.len(), 15);
}

#[tokio::test]
async fn looping_next_link_terminates() {
    let addr = Arc::new(Mutex::new(String::new()));
    let a2 = addr.clone();
    let srv = serve(move |_, _, _| {
        json_resp(200, json!({"value": [{"id": "x"}], "@odata.nextLink": format!("{}/v1.0/loop", a2.lock().unwrap())}))
    })
    .await;
    *addr.lock().unwrap() = srv.base.clone();
    let all = tokio::time::timeout(Duration::from_secs(10), srv.client("at").get_all("/loop", &[], 100_000))
        .await
        .expect("must not loop forever")
        .unwrap();
    assert_eq!(all.len(), 1, "same link is fetched once");
}

#[tokio::test]
async fn foreign_next_link_never_gets_the_token() {
    let foreign = serve(|_, h, _| json_resp(200, json!({"value": [], "auth": bearer(h)}))).await;
    let foreign_base = foreign.base.clone();
    let srv = serve(move |_, _, _| json_resp(200, json!({"value": [{"id": 1}], "@odata.nextLink": format!("{foreign_base}/v1.0/steal")})))
        .await;
    let e = srv.client("at-secret").get_all("/items", &[], 100).await.unwrap_err();
    assert!(e.to_string().contains("Unerwartete Adresse"), "{e}");
    assert_eq!(foreign.hits(), 0, "token must not leave the Graph host");
}

// ------------------------------------------------------------ service level (fake)

fn now() -> chrono::DateTime<chrono::Utc> {
    chrono::Utc::now()
}

#[tokio::test]
async fn expired_token_during_parallel_calendar_load_refreshes_once() {
    let ctx = setup_at(now()).await;
    let cals: Vec<Calendar> = call(&ctx.svc, "list_calendars", json!({"accountId": ctx.account})).await;
    assert!(cals.len() >= 2);
    let before = ctx.fake.state.token_requests();
    ctx.fake.state.expire_tokens();
    let ids: Vec<String> = cals.iter().map(|c| c.id.clone()).collect();
    let _: Vec<Event> = call(
        &ctx.svc,
        "list_events",
        json!({"accountId": ctx.account, "calendarIds": ids, "start": "2026-01-01T00:00:00Z", "end": "2026-02-01T00:00:00Z"}),
    )
    .await;
    assert_eq!(ctx.fake.state.token_requests() - before, 1);
}

#[tokio::test]
async fn revoked_session_reports_relogin_and_keeps_cache() {
    let ctx = setup_at(now()).await;
    let inbox = ctx.folder("inbox").await;
    let cached = ctx.list(&inbox.id).await.total;
    ctx.fake.state.revoke_refresh_tokens();
    ctx.fake.state.expire_tokens();
    let err = call_err(&ctx.svc, "sync_folder", json!({"mailboxId": ctx.account, "folderId": inbox.id})).await;
    assert!(err.contains("Anmeldung") && err.contains("erneut anmelden"), "{err}");
    // The local cache is untouched.
    assert_eq!(ctx.list(&inbox.id).await.total, cached);
}

#[tokio::test]
async fn transient_faults_are_absorbed() {
    let ctx = setup_at(now()).await;
    for status in [503, 504, 429] {
        ctx.fake.state.inject_fault(graph_fake::Fault::status(status).retry_after("0"));
        let cals: Vec<Calendar> = call(&ctx.svc, "list_calendars", json!({"accountId": ctx.account})).await;
        assert!(!cals.is_empty(), "{status}");
    }
    // A 500 is not retried and surfaces as error; nothing is cached as empty.
    ctx.fake.state.inject_fault(graph_fake::Fault::status(500));
    let err = call_err(&ctx.svc, "list_categories", json!({"accountId": ctx.account})).await;
    assert!(err.contains("500"), "{err}");
    let cats: Vec<Category> = call(&ctx.svc, "list_categories", json!({"accountId": ctx.account})).await;
    assert!(!cats.is_empty());
}

#[tokio::test]
async fn mail_delta_removed_items_and_expired_sync_state() {
    let ctx = setup_at(now()).await;
    let inbox = ctx.folder("inbox").await;
    let first = ctx.list(&inbox.id).await;
    let victim = first.items[0].id.clone();
    let kept = first.items[1].id.clone();
    assert!(ctx.fake.state.delete_message(ME, &victim));
    let r = ctx.sync(&inbox.id).await;
    assert_eq!(r.removed, 1);
    assert!(!ctx.list(&inbox.id).await.items.iter().any(|m| m.id == victim));

    // Delta token expires (410): full resync, nothing lost, new mail found.
    ctx.fake.state.expire_delta_tokens();
    let new_id = ctx.fake.state.inject_incoming(ME, "inbox", json!({"subject": "Nach Resync"}));
    let r = ctx.sync(&inbox.id).await;
    assert!(r.added >= 1, "{r:?}");
    let page = ctx.list(&inbox.id).await;
    assert!(page.items.iter().any(|m| m.id == new_id));
    assert!(page.items.iter().any(|m| m.id == kept));
    // And incremental sync works again afterwards.
    let r = ctx.sync(&inbox.id).await;
    assert_eq!((r.added, r.updated, r.removed), (0, 0, 0));
}

#[tokio::test]
async fn mail_resync_after_expired_delta_drops_deleted_mails() {
    let ctx = setup_at(now()).await;
    let inbox = ctx.folder("inbox").await;
    let victim = ctx.list(&inbox.id).await.items[0].id.clone();
    // Deleted elsewhere while the delta token expires.
    assert!(ctx.fake.state.delete_message(ME, &victim));
    ctx.fake.state.expire_delta_tokens();
    ctx.sync(&inbox.id).await;
    assert!(!ctx.list(&inbox.id).await.items.iter().any(|m| m.id == victim), "stale mail stays in the cache");
}

#[tokio::test]
async fn large_folder_is_paged_completely() {
    let ctx = setup_at(now()).await;
    let inbox = ctx.folder("inbox").await;
    let before = ctx.list(&inbox.id).await.total;
    for i in 0..230 {
        ctx.fake.state.inject_incoming(ME, "inbox", json!({"subject": format!("Massenmail {i}"), "isRead": true}));
    }
    let r = ctx.sync(&inbox.id).await;
    assert_eq!(r.added, 230);
    let page = ctx.list(&inbox.id).await;
    assert_eq!(page.total, before + 230);
    let mut subjects: Vec<&str> = page.items.iter().map(|m| m.subject.as_str()).filter(|s| s.starts_with("Massenmail")).collect();
    subjects.sort();
    subjects.dedup();
    assert_eq!(subjects.len(), 230, "no duplicates, nothing missing");
}

#[tokio::test]
async fn throttled_delta_page_is_retried_mid_sync() {
    let ctx = setup_at(now()).await;
    let inbox = ctx.folder("inbox").await;
    for i in 0..60 {
        ctx.fake.state.inject_incoming(ME, "inbox", json!({"subject": format!("Welle {i}"), "isRead": true}));
    }
    // Two throttled answers on the delta endpoint while paging.
    ctx.fake.state.inject_fault(graph_fake::Fault::status(429).retry_after("0").on_path("/delta"));
    ctx.fake.state.inject_fault(graph_fake::Fault::status(503).retry_after("0").on_path("/delta"));
    let r = ctx.sync(&inbox.id).await;
    assert_eq!(r.added, 60);
}

/// App restart with a stored Microsoft 365 login: the restored client starts
/// with an expired access token and must refresh it – not panic (0.3.0 crashed
/// with "`DateTime - TimeDelta` overflowed" and the UI stayed empty).
#[tokio::test]
async fn restart_with_stored_login_syncs() {
    use mailcore::host::NullHost;
    use mailcore::tokens::{MemoryTokenStore, TokenStore};
    use mailcore::{Service, ServiceOptions};

    let fake = graph_fake::spawn(graph_fake::FakeOptions { seed_demo: true, now: chrono::Utc::now() }).await;
    let dir = tempfile::tempdir().unwrap();
    let tokens: std::sync::Arc<dyn TokenStore> = std::sync::Arc::new(MemoryTokenStore::default());
    let opts = || ServiceOptions {
        data_dir: dir.path().to_path_buf(),
        graph_base: format!("{}/v1.0", fake.base_url),
        authority: fake.base_url.clone(),
        demo: true,
        token_store: tokens.clone(),
    };
    let first = Service::new(opts(), std::sync::Arc::new(NullHost::default())).unwrap();
    first.ensure_demo_account().await.unwrap();
    drop(first);

    let svc = Service::new(opts(), std::sync::Arc::new(NullHost::default())).unwrap();
    let accounts: Vec<Account> = call(&svc, "list_accounts", json!({})).await;
    let mb = accounts[0].id.clone();
    let folders = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        svc.dispatch("sync_folders", json!({"mailboxId": mb})),
    )
    .await
    .expect("sync_folders hangs")
    .expect("sync_folders failed");
    assert!(folders.as_array().is_some_and(|f| !f.is_empty()));
}
