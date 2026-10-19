//! Microsoft Entra v2.0 authorize/token endpoints (authorization code + PKCE, refresh).

use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Form;
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::state::SharedState;
use crate::util::b64url;

fn oauth_error(error: &str, desc: &str, codes: &[u32]) -> Response {
    (
        StatusCode::BAD_REQUEST,
        axum::Json(json!({
            "error": error,
            "error_description": desc,
            "error_codes": codes,
            "timestamp": chrono::Utc::now().format("%Y-%m-%d %H:%M:%SZ").to_string(),
            "trace_id": uuid::Uuid::new_v4().to_string(),
            "correlation_id": uuid::Uuid::new_v4().to_string(),
        })),
    )
        .into_response()
}

fn page(status: StatusCode, msg: &str) -> Response {
    (status, axum::response::Html(format!("<html><body><h1>Anmeldung fehlgeschlagen</h1><p>{}</p></body></html>", crate::util::escape_html(msg)))).into_response()
}

pub async fn authorize(
    State(st): State<SharedState>,
    Path(_tenant): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let Some(client_id) = q.get("client_id").filter(|c| !c.is_empty()) else {
        return page(StatusCode::BAD_REQUEST, "AADSTS900144: The request body must contain the following parameter: 'client_id'.");
    };
    let Some(redirect_uri) = q.get("redirect_uri").filter(|c| !c.is_empty()) else {
        return page(StatusCode::BAD_REQUEST, "AADSTS900144: missing 'redirect_uri'.");
    };
    if q.get("response_type").map(String::as_str) != Some("code") {
        return page(StatusCode::BAD_REQUEST, "AADSTS900144: response_type must be 'code'.");
    }
    let challenge = q.get("code_challenge").cloned();
    if challenge.is_some() && q.get("code_challenge_method").map(String::as_str) != Some("S256") {
        return page(StatusCode::BAD_REQUEST, "AADSTS501481: The Code_Verifier does not match the code_challenge supplied in the authorization request (only S256 is supported).");
    }
    let scope = q.get("scope").cloned().unwrap_or_default();
    let code = format!("fake-code-{}", uuid::Uuid::new_v4().simple());
    st.lock().auth.codes.insert(code.clone(), (client_id.clone(), redirect_uri.clone(), challenge, scope));

    let mut target = match url::Url::parse(redirect_uri) {
        Ok(u) => u,
        Err(_) => return page(StatusCode::BAD_REQUEST, "AADSTS90102: 'redirect_uri' value must be a valid absolute URI."),
    };
    {
        let mut qp = target.query_pairs_mut();
        qp.append_pair("code", &code);
        if let Some(state) = q.get("state") {
            qp.append_pair("state", state);
        }
        qp.append_pair("session_state", &uuid::Uuid::new_v4().to_string());
    }
    (StatusCode::FOUND, [(header::LOCATION, target.to_string())]).into_response()
}

fn granted_scope(requested: &str) -> String {
    let s: Vec<&str> = requested
        .split_whitespace()
        .filter(|s| !matches!(s.to_ascii_lowercase().as_str(), "offline_access" | "openid" | "profile" | "email"))
        .map(|s| s.rsplit('/').next().unwrap_or(s))
        .collect();
    if s.is_empty() {
        "User.Read".to_string()
    } else {
        s.join(" ")
    }
}

pub async fn token(State(st): State<SharedState>, Path(_tenant): Path<String>, Form(f): Form<HashMap<String, String>>) -> Response {
    let mut s = st.lock();
    let scope = match f.get("grant_type").map(String::as_str) {
        Some("authorization_code") => {
            let Some(code) = f.get("code") else {
                return oauth_error("invalid_request", "AADSTS900144: The request body must contain the following parameter: 'code'.", &[900144]);
            };
            let Some((client_id, redirect_uri, challenge, scope)) = s.auth.codes.remove(code) else {
                return oauth_error("invalid_grant", "AADSTS70008: The provided authorization code or refresh token has expired or was already used.", &[70008]);
            };
            if f.get("client_id") != Some(&client_id) {
                return oauth_error("invalid_client", "AADSTS700016: client_id does not match the authorization request.", &[700016]);
            }
            if f.get("redirect_uri") != Some(&redirect_uri) {
                return oauth_error("invalid_grant", "AADSTS50148: The redirect_uri does not match the redirect_uri of the authorization request.", &[50148]);
            }
            if let Some(ch) = challenge {
                let verifier = f.get("code_verifier").map(String::as_str).unwrap_or("");
                let computed = b64url(&Sha256::digest(verifier.as_bytes()));
                if computed != ch {
                    return oauth_error("invalid_grant", "AADSTS501481: The Code_Verifier does not match the code_challenge supplied in the authorization request.", &[501481]);
                }
            }
            scope
        }
        Some("refresh_token") => {
            let rt = f.get("refresh_token").map(String::as_str).unwrap_or("");
            match s.auth.refresh.get(rt) {
                Some(scope) => scope.clone(),
                None => {
                    return oauth_error("invalid_grant", "AADSTS70000: Provided grant is invalid or malformed.", &[70000]);
                }
            }
        }
        _ => return oauth_error("unsupported_grant_type", "AADSTS70003: The app requested an unsupported grant type.", &[70003]),
    };
    let access = s.issue_access_token();
    let refresh = format!("fake-rt-{}", uuid::Uuid::new_v4().simple());
    s.auth.refresh.insert(refresh.clone(), scope.clone());
    axum::Json(json!({
        "token_type": "Bearer",
        "scope": granted_scope(&scope),
        "expires_in": 3600,
        "ext_expires_in": 3600,
        "access_token": access,
        "refresh_token": refresh,
    }))
    .into_response()
}
