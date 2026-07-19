use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::{
    body::Body,
    extract::{Request, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Json, Response},
    routing::{get, post},
    Router,
};
use base64::Engine;
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tower_http::{limit::RequestBodyLimitLayer, timeout::TimeoutLayer};

use crate::config::Config;
use crate::state::{AcmeRecords, AddError};

const MAX_TXT_VALUE_LEN: usize = 255;
pub const MAX_REQUEST_BODY_LEN: usize = 1024;
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_CONCURRENT_REQUESTS: usize = 64;
const REQUEST_RATE_PER_SECOND: f64 = 20.0;
const REQUEST_RATE_BURST: usize = 100;

#[derive(Clone)]
pub struct ApiState {
    pub config: Config,
    pub acme: AcmeRecords,
    request_gate: RequestGate,
}

#[derive(Clone)]
struct RequestGate {
    concurrency: Arc<Semaphore>,
    rate: Arc<Mutex<TokenBucket>>,
}

struct TokenBucket {
    tokens: f64,
    last_refill: Instant,
    rate_per_second: f64,
    burst: f64,
}

impl RequestGate {
    fn new(max_concurrent: usize, rate_per_second: f64, burst: usize) -> Self {
        Self {
            concurrency: Arc::new(Semaphore::new(max_concurrent)),
            rate: Arc::new(Mutex::new(TokenBucket {
                tokens: burst as f64,
                last_refill: Instant::now(),
                rate_per_second,
                burst: burst as f64,
            })),
        }
    }

    fn check_rate(&self) -> bool {
        let mut bucket = self.rate.lock().unwrap_or_else(|error| error.into_inner());
        let now = Instant::now();
        let elapsed = now.duration_since(bucket.last_refill).as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * bucket.rate_per_second).min(bucket.burst);
        bucket.last_refill = now;
        if bucket.tokens < 1.0 {
            return false;
        }
        bucket.tokens -= 1.0;
        true
    }

    fn try_acquire(&self) -> Option<OwnedSemaphorePermit> {
        self.concurrency.clone().try_acquire_owned().ok()
    }
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
    let rate_burst = REQUEST_RATE_BURST.max(config.max_tokens.saturating_mul(2));
    router_with_limits(
        config,
        acme,
        MAX_CONCURRENT_REQUESTS,
        REQUEST_RATE_PER_SECOND,
        rate_burst,
        REQUEST_TIMEOUT,
    )
}

fn router_with_limits(
    config: Config,
    acme: AcmeRecords,
    max_concurrent: usize,
    rate_per_second: f64,
    rate_burst: usize,
    request_timeout: Duration,
) -> Router {
    let state = ApiState {
        config,
        acme,
        request_gate: RequestGate::new(max_concurrent, rate_per_second, rate_burst),
    };
    let protected = Router::new()
        .route("/present", post(present))
        .route("/cleanup", post(cleanup))
        .route_layer(RequestBodyLimitLayer::new(MAX_REQUEST_BODY_LEN))
        .route_layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            request_timeout,
        ))
        // Added last so authentication and admission happen before any body is read.
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            authorize_and_admit,
        ));

    Router::new()
        .route("/health", get(health))
        .merge(protected)
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
    Json(body): Json<PresentRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    let name = validate_request(&body, &state.config)?;
    state
        .acme
        .add(name.clone(), body.value)
        .await
        .map_err(|error| match error {
            AddError::TokenLimitReached => service_unavailable("active token limit reached"),
            AddError::DnsWireCapacityReached => {
                service_unavailable("active TXT records have reached DNS TCP capacity")
            }
        })?;
    tracing::info!("present: {name}");

    Ok(Json(serde_json::json!({})))
}

async fn cleanup(
    State(state): State<ApiState>,
    Json(body): Json<PresentRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    let name = validate_request(&body, &state.config)?;
    state.acme.remove(&name, &body.value).await;
    tracing::info!("cleanup: {name}");

    Ok(Json(serde_json::json!({})))
}

// --- authentication and request admission ---

async fn authorize_and_admit(
    State(state): State<ApiState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    if let Err(message) = check_basic_auth(request.headers(), &state.config) {
        return unauthorized(message);
    }
    if !state.request_gate.check_rate() {
        return rate_limited();
    }
    let Some(_permit) = state.request_gate.try_acquire() else {
        return service_unavailable("concurrent request limit reached").into_response();
    };
    next.run(request).await
}

fn check_basic_auth(headers: &HeaderMap, config: &Config) -> Result<(), &'static str> {
    let Some(raw) = headers.get(header::AUTHORIZATION) else {
        return Err("missing Basic auth");
    };
    let raw = raw.as_bytes();
    let Some(separator) = raw.iter().position(|byte| *byte == b' ') else {
        return Err("malformed Basic auth");
    };
    let (scheme, encoded) = raw.split_at(separator);
    if !scheme.eq_ignore_ascii_case(b"Basic") {
        return Err("missing Basic auth");
    }
    let encoded = encoded[1..]
        .iter()
        .skip_while(|byte| **byte == b' ')
        .copied()
        .collect::<Vec<_>>();
    if encoded.is_empty() || encoded.contains(&b' ') {
        return Err("malformed Basic auth");
    }
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(&encoded)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .ok_or("malformed Basic auth")?;

    // Username is ignored; only the password must match the API key.
    let (_user, pass) = decoded.split_once(':').ok_or("malformed Basic auth")?;

    if bool::from(pass.as_bytes().ct_eq(config.api_key.as_bytes())) {
        Ok(())
    } else {
        Err("invalid credentials")
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

fn unauthorized(msg: &'static str) -> Response {
    let mut response = error_response(StatusCode::UNAUTHORIZED, msg).into_response();
    response.headers_mut().insert(
        header::WWW_AUTHENTICATE,
        HeaderValue::from_static("Basic realm=\"safexip\", charset=\"UTF-8\""),
    );
    response
}

fn rate_limited() -> Response {
    let mut response =
        error_response(StatusCode::TOO_MANY_REQUESTS, "request rate limit reached").into_response();
    response
        .headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
    response
}

fn error_response(status: StatusCode, msg: &'static str) -> (StatusCode, Json<ErrorResponse>) {
    (status, Json(ErrorResponse { error: msg.into() }))
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};
    use std::time::Duration;

    use axum::body::Body;
    use axum::http::{header, Request};
    use futures_util::stream;
    use tower::ServiceExt;

    use super::*;
    use crate::wire::AcmeWireCapacity;

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
        let config = config();
        let records = AcmeRecords::new(
            Duration::from_secs(60),
            10,
            AcmeWireCapacity::from_config(&config),
        );
        router(config, records)
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
        assert_eq!(
            missing.headers().get(header::WWW_AUTHENTICATE).unwrap(),
            "Basic realm=\"safexip\", charset=\"UTF-8\""
        );

        let wrong = app()
            .oneshot(request("/present", Some(&auth("wrong")), body))
            .await
            .unwrap();
        assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn rejects_malformed_basic_authentication() {
        let body = r#"{"fqdn":"_acme-challenge.xip.test.","value":"token"}"#;
        let missing_colon = base64::engine::general_purpose::STANDARD.encode(KEY);
        let invalid_utf8 = base64::engine::general_purpose::STANDARD
            .encode([vec![0xff, b':'], KEY.as_bytes().to_vec()].concat());
        for authorization in [
            "Bearer token".to_owned(),
            "Basic !!!".to_owned(),
            format!("Basic {missing_colon}"),
            format!("Basic {invalid_utf8}"),
        ] {
            let response = app()
                .oneshot(request("/present", Some(&authorization), body))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
            assert!(response.headers().contains_key(header::WWW_AUTHENTICATE));
        }
    }

    #[tokio::test]
    async fn accepts_basic_scheme_case_insensitively() {
        let authorization = auth(KEY).replacen("Basic", "bAsIc", 1);
        let body = r#"{"fqdn":"_acme-challenge.xip.test.","value":"token"}"#;
        let response = app()
            .oneshot(request("/present", Some(&authorization), body))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn rejects_authentication_before_parsing_the_body() {
        let unauthenticated = app()
            .oneshot(request("/present", None, "not json"))
            .await
            .unwrap();
        assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

        let authenticated = app()
            .oneshot(request("/present", Some(&auth(KEY)), "not json"))
            .await
            .unwrap();
        assert_eq!(authenticated.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn rejects_oversized_bodies_with_and_without_a_known_length() {
        let oversized = "x".repeat(MAX_REQUEST_BODY_LEN + 1);
        let known = app()
            .oneshot(request("/present", Some(&auth(KEY)), &oversized))
            .await
            .unwrap();
        assert_eq!(known.status(), StatusCode::PAYLOAD_TOO_LARGE);

        let unknown_body = Body::from_stream(stream::once(async move {
            Ok::<_, std::convert::Infallible>(oversized)
        }));
        let unknown = Request::post("/present")
            .header("content-type", "application/json")
            .header("authorization", auth(KEY))
            .body(unknown_body)
            .unwrap();
        let response = app().oneshot(unknown).await.unwrap();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn allows_a_normal_concurrent_lego_burst() {
        let config = config();
        let records = AcmeRecords::new(
            Duration::from_secs(60),
            20,
            AcmeWireCapacity::from_config(&config),
        );
        let app = router(config, records.clone());
        let mut tasks = Vec::new();
        for index in 0..12 {
            let app = app.clone();
            tasks.push(tokio::spawn(async move {
                let body = serde_json::json!({
                    "fqdn": "_acme-challenge.xip.test.",
                    "value": format!("token-{index}"),
                });
                app.oneshot(request("/present", Some(&auth(KEY)), &body.to_string()))
                    .await
                    .unwrap()
                    .status()
            }));
        }
        for task in tasks {
            assert_eq!(task.await.unwrap(), StatusCode::OK);
        }
        assert_eq!(records.get("_acme-challenge.xip.test").await.len(), 12);
    }

    #[tokio::test]
    async fn rate_limit_has_a_burst_and_retry_header() {
        let config = config();
        let records = AcmeRecords::new(
            Duration::from_secs(60),
            10,
            AcmeWireCapacity::from_config(&config),
        );
        let app = router_with_limits(config, records, 2, 0.0, 2, REQUEST_TIMEOUT);
        for (index, expected) in [
            (0, StatusCode::OK),
            (1, StatusCode::OK),
            (2, StatusCode::TOO_MANY_REQUESTS),
        ] {
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
            if expected == StatusCode::TOO_MANY_REQUESTS {
                assert_eq!(response.headers().get(header::RETRY_AFTER).unwrap(), "1");
            }
        }
    }

    #[tokio::test]
    async fn times_out_an_incomplete_authenticated_body() {
        let config = config();
        let records = AcmeRecords::new(
            Duration::from_secs(60),
            10,
            AcmeWireCapacity::from_config(&config),
        );
        let app = router_with_limits(config, records, 1, 100.0, 10, Duration::from_millis(20));
        let pending =
            Body::from_stream(stream::pending::<Result<String, std::convert::Infallible>>());
        let request = Request::post("/present")
            .header("content-type", "application/json")
            .header("authorization", auth(KEY))
            .body(pending)
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::REQUEST_TIMEOUT);
    }

    #[tokio::test]
    async fn rejects_and_recovers_from_concurrency_exhaustion() {
        let config = config();
        let records = AcmeRecords::new(
            Duration::from_secs(60),
            10,
            AcmeWireCapacity::from_config(&config),
        );
        let app = router_with_limits(config, records, 1, 100.0, 10, Duration::from_secs(1));
        let pending =
            Body::from_stream(stream::pending::<Result<String, std::convert::Infallible>>());
        let held = Request::post("/present")
            .header("content-type", "application/json")
            .header("authorization", auth(KEY))
            .body(pending)
            .unwrap();
        let held_task = tokio::spawn(app.clone().oneshot(held));
        tokio::time::sleep(Duration::from_millis(10)).await;

        let body = r#"{"fqdn":"_acme-challenge.xip.test.","value":"token"}"#;
        let exhausted = app
            .clone()
            .oneshot(request("/present", Some(&auth(KEY)), body))
            .await
            .unwrap();
        assert_eq!(exhausted.status(), StatusCode::SERVICE_UNAVAILABLE);

        held_task.abort();
        let _ = held_task.await;
        let recovered = app
            .oneshot(request("/present", Some(&auth(KEY)), body))
            .await
            .unwrap();
        assert_eq!(recovered.status(), StatusCode::OK);
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
        let config = config();
        let records = AcmeRecords::new(
            Duration::from_secs(60),
            10,
            AcmeWireCapacity::from_config(&config),
        );
        let app = router(config, records.clone());
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
        let config = config();
        let records = AcmeRecords::new(
            Duration::from_secs(60),
            1,
            AcmeWireCapacity::from_config(&config),
        );
        let app = router(config, records);
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

    #[tokio::test]
    async fn reports_dns_wire_capacity_without_changing_existing_records() {
        use axum::body::to_bytes;

        let config = config();
        let records = AcmeRecords::new(
            Duration::from_secs(60),
            usize::MAX,
            AcmeWireCapacity::from_config(&config),
        );
        let app = router_with_limits(
            config,
            records.clone(),
            MAX_CONCURRENT_REQUESTS,
            REQUEST_RATE_PER_SECOND,
            1_000,
            REQUEST_TIMEOUT,
        );
        let mut accepted = Vec::new();

        loop {
            let value = format!("{:04}{}", accepted.len(), "x".repeat(251));
            let body = serde_json::json!({
                "fqdn": "_acme-challenge.xip.test.",
                "value": value,
            });
            let response = app
                .clone()
                .oneshot(request("/present", Some(&auth(KEY)), &body.to_string()))
                .await
                .unwrap();
            if response.status() == StatusCode::OK {
                accepted.push(value);
                continue;
            }

            assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
            let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
                serde_json::json!({
                    "error": "active TXT records have reached DNS TCP capacity"
                })
            );
            break;
        }

        assert_eq!(records.get("_acme-challenge.xip.test").await, accepted);
    }
}
