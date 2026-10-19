//! Small helpers shared by the route modules: Graph error envelope, request
//! wrapper, paging, `$select`, id generation and date formatting.

use axum::body::Bytes;
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use chrono::{DateTime, NaiveDateTime, SecondsFormat, Utc};
use serde_json::{json, Map, Value};

/// A Graph style error: `{"error":{"code","message","innerError":{...}}}`.
#[derive(Debug)]
pub struct GErr {
    pub status: StatusCode,
    pub code: String,
    pub message: String,
}

impl GErr {
    pub fn new(status: StatusCode, code: &str, message: impl Into<String>) -> Self {
        GErr { status, code: code.to_string(), message: message.into() }
    }
    pub fn not_found(what: &str) -> Self {
        GErr::new(
            StatusCode::NOT_FOUND,
            "ErrorItemNotFound",
            format!("The specified object was not found in the store. ({what})"),
        )
    }
    pub fn bad(message: impl Into<String>) -> Self {
        GErr::new(StatusCode::BAD_REQUEST, "ErrorInvalidRequest", message)
    }
    pub fn access_denied() -> Self {
        GErr::new(StatusCode::FORBIDDEN, "ErrorAccessDenied", "Access is denied. Check credentials and try again.")
    }
}

impl IntoResponse for GErr {
    fn into_response(self) -> Response {
        let body = json!({
            "error": {
                "code": self.code,
                "message": self.message,
                "innerError": {
                    "date": Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
                    "request-id": uuid::Uuid::new_v4().to_string(),
                    "client-request-id": uuid::Uuid::new_v4().to_string()
                }
            }
        });
        let mut resp = (self.status, axum::Json(body)).into_response();
        if self.status == StatusCode::UNAUTHORIZED {
            resp.headers_mut().insert(
                header::WWW_AUTHENTICATE,
                HeaderValue::from_static("Bearer realm=\"\", error=\"invalid_token\""),
            );
        }
        resp
    }
}

pub type GResult = Result<Response, GErr>;

pub fn json_resp(status: StatusCode, v: Value) -> Response {
    (status, axum::Json(v)).into_response()
}
pub fn ok(v: Value) -> GResult {
    Ok(json_resp(StatusCode::OK, v))
}
pub fn created(v: Value) -> GResult {
    Ok(json_resp(StatusCode::CREATED, v))
}
pub fn no_content() -> GResult {
    Ok(StatusCode::NO_CONTENT.into_response())
}
pub fn accepted() -> GResult {
    Ok(StatusCode::ACCEPTED.into_response())
}

/// Parsed Graph request (everything after `/v1.0`).
pub struct Req {
    pub method: Method,
    /// Percent-decoded path segments after `/v1.0`.
    pub segs: Vec<String>,
    /// Raw path after `/v1.0` (still encoded), used to build next links.
    pub raw_path: String,
    pub query: Vec<(String, String)>,
    pub headers: HeaderMap,
    pub body: Bytes,
    pub base_url: String,
}

impl Req {
    /// Query parameter lookup; `$top` and `top` are treated alike, case-insensitive.
    pub fn q(&self, key: &str) -> Option<&str> {
        let k = key.trim_start_matches('$').to_ascii_lowercase();
        self.query
            .iter()
            .find(|(n, _)| n.trim_start_matches('$').to_ascii_lowercase() == k)
            .map(|(_, v)| v.as_str())
    }

    pub fn json(&self) -> Result<Value, GErr> {
        if self.body.is_empty() {
            return Ok(json!({}));
        }
        serde_json::from_slice(&self.body).map_err(|e| {
            GErr::new(StatusCode::BAD_REQUEST, "RequestBodyRead", format!("Invalid JSON body: {e}"))
        })
    }

    fn prefer(&self) -> Vec<String> {
        self.headers
            .get_all("prefer")
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|s| s.split(',').map(|p| p.trim().to_string()).collect::<Vec<_>>())
            .collect()
    }

    pub fn max_page_size(&self) -> Option<usize> {
        self.prefer().iter().find_map(|p| {
            p.strip_prefix("odata.maxpagesize=").and_then(|n| n.trim().parse().ok())
        })
    }

    /// `Prefer: outlook.timezone="Europe/Berlin"` → `Europe/Berlin`.
    pub fn prefer_tz(&self) -> Option<String> {
        self.prefer().iter().find_map(|p| {
            p.strip_prefix("outlook.timezone=").map(|v| v.trim().trim_matches('"').to_string())
        })
    }

    pub fn top(&self) -> Option<usize> {
        self.q("top").and_then(|t| t.parse().ok())
    }
    pub fn skip(&self) -> usize {
        self.q("skip").and_then(|t| t.parse().ok()).unwrap_or(0)
    }

    /// Absolute URL to the same resource with some query params replaced/removed.
    pub fn link_with(&self, set: &[(&str, String)], remove: &[&str]) -> String {
        let mut q: Vec<(String, String)> = self
            .query
            .iter()
            .filter(|(k, _)| {
                let kk = k.trim_start_matches('$').to_ascii_lowercase();
                !set.iter().any(|(s, _)| s.trim_start_matches('$').eq_ignore_ascii_case(&kk))
                    && !remove.iter().any(|r| r.trim_start_matches('$').eq_ignore_ascii_case(&kk))
            })
            .cloned()
            .collect();
        for (k, v) in set {
            q.push((k.to_string(), v.clone()));
        }
        let qs = url::form_urlencoded::Serializer::new(String::new()).extend_pairs(q.iter()).finish();
        if qs.is_empty() {
            format!("{}/v1.0{}", self.base_url, self.raw_path)
        } else {
            format!("{}/v1.0{}?{}", self.base_url, self.raw_path, qs)
        }
    }
}

/// Apply `$top`/`$skip`/`Prefer: odata.maxpagesize` paging and wrap in `{value, @odata.nextLink}`.
pub fn paged(req: &Req, items: Vec<Value>, default_size: usize) -> Value {
    let size = req.top().or(req.max_page_size()).unwrap_or(default_size).max(1);
    let skip = req.skip();
    let total = items.len();
    let page: Vec<Value> = items.into_iter().skip(skip).take(size).collect();
    let mut out = json!({ "@odata.context": format!("{}/v1.0/$metadata", req.base_url), "value": page });
    if skip + size < total {
        out["@odata.nextLink"] = Value::String(req.link_with(&[("$skip", (skip + size).to_string())], &[]));
    }
    out
}

/// Loose `$select` support: keeps listed properties plus id / OData annotations.
pub fn apply_select(v: Value, select: Option<&str>) -> Value {
    let Some(sel) = select else { return v };
    let wanted: Vec<String> = sel.split(',').map(|s| s.trim().to_ascii_lowercase()).filter(|s| !s.is_empty()).collect();
    if wanted.is_empty() {
        return v;
    }
    match v {
        Value::Object(m) => {
            let filtered: Map<String, Value> = m
                .into_iter()
                .filter(|(k, _)| k == "id" || k.starts_with('@') || wanted.contains(&k.to_ascii_lowercase()))
                .collect();
            Value::Object(filtered)
        }
        other => other,
    }
}

pub fn new_id(prefix: &str) -> String {
    let u = uuid::Uuid::new_v4().simple().to_string().to_ascii_uppercase();
    format!("{prefix}{}", &u[..28])
}

pub fn iso(dt: DateTime<Utc>) -> String {
    dt.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Graph dateTimeTimeZone format: `2026-10-20T07:00:00.0000000`.
pub fn graph_dt(n: NaiveDateTime) -> String {
    n.format("%Y-%m-%dT%H:%M:%S.0000000").to_string()
}

pub fn parse_naive(s: &str) -> Option<NaiveDateTime> {
    let s = s.trim().trim_end_matches('Z');
    for f in ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%dT%H:%M:%S", "%Y-%m-%dT%H:%M", "%Y-%m-%d %H:%M:%S"] {
        if let Ok(d) = NaiveDateTime::parse_from_str(s, f) {
            return Some(d);
        }
    }
    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok().and_then(|d| d.and_hms_opt(0, 0, 0))
}

/// Parses ISO timestamps with or without offset (no offset = UTC).
pub fn parse_utc(s: &str) -> Option<DateTime<Utc>> {
    if let Ok(d) = DateTime::parse_from_rfc3339(s.trim()) {
        return Some(d.with_timezone(&Utc));
    }
    parse_naive(s).map(|n| n.and_utc())
}

pub fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}
pub fn unb64(s: &str) -> Option<Vec<u8>> {
    base64::engine::general_purpose::STANDARD.decode(s.trim()).ok()
}
pub fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}
pub fn unb64url(s: &str) -> Option<Vec<u8>> {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(s.trim()).ok()
}

/// Strips tags and collapses whitespace – used for bodyPreview.
pub fn html_to_text(html: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    let lower = html.to_ascii_lowercase();
    // drop <style>/<head> content
    let mut skip_until: Option<usize> = None;
    let chars: Vec<(usize, char)> = html.char_indices().collect();
    for &(i, c) in &chars {
        if let Some(end) = skip_until {
            if i < end {
                continue;
            }
            skip_until = None;
        }
        if c == '<' {
            for tag in ["<style", "<head", "<script"] {
                if lower[i..].starts_with(tag) {
                    let close = format!("</{}>", &tag[1..]);
                    if let Some(p) = lower[i..].find(&close) {
                        skip_until = Some(i + p + close.len());
                    }
                }
            }
            if skip_until.is_some() {
                continue;
            }
            in_tag = true;
            out.push(' ');
            continue;
        }
        if c == '>' && in_tag {
            in_tag = false;
            continue;
        }
        if !in_tag {
            out.push(c);
        }
    }
    let out = out
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&");
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn preview(body: &Value) -> String {
    let content = body.get("content").and_then(Value::as_str).unwrap_or("");
    let text = if body.get("contentType").and_then(Value::as_str).unwrap_or("text").eq_ignore_ascii_case("html") {
        html_to_text(content)
    } else {
        content.split_whitespace().collect::<Vec<_>>().join(" ")
    };
    text.chars().take(255).collect()
}

pub fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Shallow merge of `patch` into `target` (objects only).
pub fn merge(target: &mut Value, patch: &Value) {
    if let (Some(t), Some(p)) = (target.as_object_mut(), patch.as_object()) {
        for (k, v) in p {
            t.insert(k.clone(), v.clone());
        }
    }
}

pub fn email(name: &str, addr: &str) -> Value {
    json!({ "emailAddress": { "name": name, "address": addr } })
}

pub fn email_addr(v: &Value) -> String {
    v.pointer("/emailAddress/address").and_then(Value::as_str).unwrap_or("").to_ascii_lowercase()
}
