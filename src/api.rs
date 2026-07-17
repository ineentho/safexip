use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::Json,
    routing::{get, post},
    Router,
};
use base64::Engine;
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;

use crate::config::Config;
use crate::state::{AcmeRecords, AddError};

const MAX_TXT_VALUE_LEN: usize = 255;

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

pub fn router(config: Config, acme: AcmeRecords) -> Router {
    let state = ApiState { config, acme };
    Router::new()
        .route("/health", get(health))
        .route("/present", post(present))
        .route("/cleanup", post(cleanup))
        .with_state(state)
}

async fn health(State(state): State<ApiState>) -> Json<StatusResponse> {
    Json(StatusResponse {
        status: "ok",
        domain: state.config.domain,
    })
}

// --- lego --dns httpreq protocol ---

async fn present(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<PresentRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    check_basic_auth(&headers, &state.config)?;

    let name = validate_request(&body, &state.config)?;
    state
        .acme
        .add(name.clone(), body.value)
        .await
        .map_err(|error| match error {
            AddError::CapacityReached => service_unavailable("active token limit reached"),
        })?;
    tracing::info!("present: {name}");

    Ok(Json(serde_json::json!({})))
}

async fn cleanup(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<PresentRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    check_basic_auth(&headers, &state.config)?;

    let name = validate_request(&body, &state.config)?;
    state.acme.remove(&name, &body.value).await;
    tracing::info!("cleanup: {name}");

    Ok(Json(serde_json::json!({})))
}

// --- auth ---

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

    // Username is ignored; only the password must match the API key.
    let (_user, pass) = decoded.split_once(':').unwrap_or(("", &decoded));

    if bool::from(pass.as_bytes().ct_eq(config.api_key.as_bytes())) {
        Ok(())
    } else {
        Err(unauthorized("invalid credentials"))
    }
}

fn validate_request(
    body: &PresentRequest,
    config: &Config,
) -> Result<String, (StatusCode, Json<ErrorResponse>)> {
    let name = body.fqdn.trim_end_matches('.').to_ascii_lowercase();
    if name != config.acme_name() {
        return Err(bad_request("fqdn is not this zone's ACME challenge name"));
    }
    if body.value.is_empty() {
        return Err(bad_request("token must not be empty"));
    }
    if body.value.len() > MAX_TXT_VALUE_LEN {
        return Err(bad_request("token exceeds the DNS TXT string limit"));
    }
    Ok(name)
}

fn bad_request(msg: &'static str) -> (StatusCode, Json<ErrorResponse>) {
    error_response(StatusCode::BAD_REQUEST, msg)
}

fn service_unavailable(msg: &'static str) -> (StatusCode, Json<ErrorResponse>) {
    error_response(StatusCode::SERVICE_UNAVAILABLE, msg)
}

fn unauthorized(msg: &'static str) -> (StatusCode, Json<ErrorResponse>) {
    error_response(StatusCode::UNAUTHORIZED, msg)
}

fn error_response(status: StatusCode, msg: &'static str) -> (StatusCode, Json<ErrorResponse>) {
    (status, Json(ErrorResponse { error: msg.into() }))
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};
    use std::time::Duration;

    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    use super::*;

    const KEY: &str = "0123456789abcdef0123456789abcdef";

    fn config() -> Config {
        Config {
            domain: "xip.test".into(),
            ns_hostname: "ns1.xip.test".into(),
            ns_hostname2: "ns2.xip.test".into(),
            dns_bind: IpAddr::V4(Ipv4Addr::LOCALHOST),
            dns_port: 5353,
            api_bind: IpAddr::V4(Ipv4Addr::LOCALHOST),
            api_port: 8080,
            ns_ip: Ipv4Addr::LOCALHOST,
            api_key: KEY.into(),
            txt_ttl: 60,
            default_ttl: 60,
            token_lifetime: 600,
            max_tokens: 10,
        }
    }

    fn app() -> Router {
        router(config(), AcmeRecords::new(Duration::from_secs(60), 10))
    }

    fn request(path: &str, auth: Option<&str>, body: &str) -> Request<Body> {
        let mut builder = Request::post(path).header("content-type", "application/json");
        if let Some(auth) = auth {
            builder = builder.header("authorization", auth);
        }
        builder.body(Body::from(body.to_owned())).unwrap()
    }

    fn auth(password: &str) -> String {
        let encoded = base64::engine::general_purpose::STANDARD.encode(format!("user:{password}"));
        format!("Basic {encoded}")
    }

    #[tokio::test]
    async fn rejects_missing_and_incorrect_authentication() {
        let body = r#"{"fqdn":"_acme-challenge.xip.test.","value":"token"}"#;
        let missing = app()
            .oneshot(request("/present", None, body))
            .await
            .unwrap();
        assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);

        let wrong = app()
            .oneshot(request("/present", Some(&auth("wrong")), body))
            .await
            .unwrap();
        assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn accepts_the_configured_challenge_name_case_insensitively() {
        let body = r#"{"fqdn":"_AcMe-Challenge.XIP.TEST.","value":"token"}"#;
        let response = app()
            .oneshot(request("/present", Some(&auth(KEY)), body))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn rejects_names_that_the_dns_server_cannot_answer() {
        let body = r#"{"fqdn":"_acme-challenge.other.test.","value":"token"}"#;
        let response = app()
            .oneshot(request("/present", Some(&auth(KEY)), body))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn rejects_empty_and_oversized_tokens() {
        for value in [String::new(), "x".repeat(MAX_TXT_VALUE_LEN + 1)] {
            let body = serde_json::json!({
                "fqdn": "_acme-challenge.xip.test.",
                "value": value,
            });
            let response = app()
                .oneshot(request("/present", Some(&auth(KEY)), &body.to_string()))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        }
    }

    #[tokio::test]
    async fn cleanup_removes_only_the_requested_token() {
        let records = AcmeRecords::new(Duration::from_secs(60), 10);
        let app = router(config(), records.clone());
        for value in ["token-one", "token-two"] {
            let body = serde_json::json!({
                "fqdn": "_acme-challenge.xip.test.",
                "value": value,
            });
            let response = app
                .clone()
                .oneshot(request("/present", Some(&auth(KEY)), &body.to_string()))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
        }

        let cleanup = r#"{"fqdn":"_acme-challenge.xip.test.","value":"token-one"}"#;
        let response = app
            .oneshot(request("/cleanup", Some(&auth(KEY)), cleanup))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(records.get("_acme-challenge.xip.test").await, ["token-two"]);
    }

    #[tokio::test]
    async fn reports_capacity_exhaustion() {
        let app = router(config(), AcmeRecords::new(Duration::from_secs(60), 1));
        for (index, expected) in [(1, StatusCode::OK), (2, StatusCode::SERVICE_UNAVAILABLE)] {
            let body = serde_json::json!({
                "fqdn": "_acme-challenge.xip.test.",
                "value": format!("token-{index}"),
            });
            let response = app
                .clone()
                .oneshot(request("/present", Some(&auth(KEY)), &body.to_string()))
                .await
                .unwrap();
            assert_eq!(response.status(), expected);
        }
    }
}
