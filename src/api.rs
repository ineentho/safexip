use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::Json,
    routing::{get, post},
    Router,
};
use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::state::AcmeRecords;

#[derive(Clone)]
pub struct ApiState {
    pub config: Config,
    pub acme: AcmeRecords,
}

#[derive(Serialize)]
struct StatusResponse {
    status: &'static str,
    domain: String,
}

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
}

#[derive(Deserialize)]
struct PresentRequest {
    fqdn: String,
    value: String,
}

#[derive(Deserialize)]
struct SetTxtRequest {
    name: String,
    value: String,
}

#[derive(Deserialize)]
struct DeleteTxtRequest {
    name: String,
}

pub fn router(config: Config, acme: AcmeRecords) -> Router {
    let state = ApiState { config, acme };
    Router::new()
        .route("/health", get(health))
        .route("/present", post(present))
        .route("/cleanup", post(cleanup))
        .route("/v1/txt", post(set_txt).delete(delete_txt))
        .with_state(state)
}

async fn health(State(state): State<ApiState>) -> Json<StatusResponse> {
    Json(StatusResponse {
        status: "ok",
        domain: state.config.domain,
    })
}

// --- httpreq protocol (lego --dns httpreq) ---

async fn present(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<PresentRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    check_basic_auth(&headers, &state.config)?;

    // fqdn is like "_acme-challenge.xip.henrikdev.com." (trailing dot)
    let name = body.fqdn.trim_end_matches('.');
    state.acme.add(name.to_string(), body.value.clone()).await;
    tracing::info!("present: {name}");

    Ok(Json(serde_json::json!({})))
}

async fn cleanup(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<PresentRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    check_basic_auth(&headers, &state.config)?;

    let name = body.fqdn.trim_end_matches('.');
    state.acme.remove(name, &body.value).await;
    tracing::info!("cleanup: {name}");

    Ok(Json(serde_json::json!({})))
}

// --- admin endpoints ---

async fn set_txt(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<SetTxtRequest>,
) -> Result<Json<StatusResponse>, (StatusCode, Json<ErrorResponse>)> {
    check_admin_auth(&headers, &state.config)?;

    if !body.name.ends_with(&format!(".{}", state.config.domain))
        && body.name != state.config.domain
    {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!("name must be within domain {}", state.config.domain),
            }),
        ));
    }

    state.acme.add(body.name.clone(), body.value).await;
    tracing::info!("set TXT: {}", body.name);

    Ok(Json(StatusResponse {
        status: "set",
        domain: state.config.domain,
    }))
}

async fn delete_txt(
    State(state): State<ApiState>,
    headers: HeaderMap,
    axum::extract::Query(params): axum::extract::Query<DeleteTxtRequest>,
) -> Result<Json<StatusResponse>, (StatusCode, Json<ErrorResponse>)> {
    check_admin_auth(&headers, &state.config)?;

    state.acme.delete_all(&params.name).await;
    tracing::info!("deleted TXT: {}", params.name);

    Ok(Json(StatusResponse {
        status: "deleted",
        domain: state.config.domain,
    }))
}

// --- auth helpers ---

fn check_admin_auth(
    headers: &HeaderMap,
    config: &Config,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    let key = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));

    match key {
        Some(k) if k == config.api_key => Ok(()),
        _ => Err(unauthorized("missing or invalid admin API key")),
    }
}

fn check_basic_auth(
    headers: &HeaderMap,
    config: &Config,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    let raw = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Basic "));

    let Some(decoded) = raw.and_then(|r| {
        base64::engine::general_purpose::STANDARD
            .decode(r)
            .ok()
            .and_then(|b| String::from_utf8(b).ok())
    }) else {
        return Err(unauthorized("missing Basic auth"));
    };

    let (_user, pass) = decoded.split_once(':').unwrap_or(("", &decoded));

    if pass == config.api_key {
        Ok(())
    } else {
        Err(unauthorized("invalid credentials"))
    }
}

fn unauthorized(msg: &'static str) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::UNAUTHORIZED,
        Json(ErrorResponse {
            error: msg.into(),
        }),
    )
}
