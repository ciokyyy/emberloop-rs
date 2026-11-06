#![allow(clippy::unwrap_used, clippy::expect_used)]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use emberloop_daemon::codex::{CodexEndpoints, CodexLogin};
use serde_json::json;

#[derive(Clone, Default)]
struct Counts {
    tokens: Arc<AtomicUsize>,
}

async fn usercode() -> Json<serde_json::Value> {
    Json(json!({
        "device_auth_id": "device-1",
        "user_code": "TEST-CODE",
        "interval": "0",
        "expires_at": "2030-01-01T00:00:00Z"
    }))
}

async fn token(State(counts): State<Counts>) -> Response {
    if counts.tokens.fetch_add(1, Ordering::SeqCst) == 0 {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error": {"code": "deviceauth_authorization_pending"}})),
        )
            .into_response();
    }
    Json(json!({
        "authorization_code": "auth-code",
        "code_verifier": "verifier"
    }))
    .into_response()
}

async fn exchange(body: String) -> Response {
    assert!(
        body.contains("grant_type=authorization_code"),
        "exchange body was {body:?}"
    );
    assert!(body.contains("redirect_uri=https%3A%2F%2Fauth.openai.com%2Fdeviceauth%2Fcallback"));
    Json(json!({
        "access_token": "access",
        "refresh_token": "refresh",
        "expires_in": 3600
    }))
    .into_response()
}

async fn server() -> (String, Counts) {
    let counts = Counts::default();
    let app = Router::new()
        .route("/usercode", post(usercode))
        .route("/token", post(token))
        .route("/exchange", post(exchange))
        .with_state(counts.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (base, counts)
}

async fn wait(login: &CodexLogin, id: &str) -> api::providers::LoginView {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let view = login.status(id).await.unwrap();
        if view.state != "running" {
            return view;
        }
        assert!(Instant::now() < deadline, "login did not finish");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
async fn device_code_login_writes_pi_auth_and_can_sign_out() {
    let dir = tempfile::tempdir().unwrap();
    let auth = dir.path().join("pi/agent/auth.json");
    let (base, _) = server().await;
    let login = CodexLogin::new(&auth).with_endpoints(CodexEndpoints {
        user_code: format!("{base}/usercode"),
        device_token: format!("{base}/token"),
        exchange: format!("{base}/exchange"),
    });

    let started = login.start("").await.unwrap();
    assert_eq!(started.user_code, "TEST-CODE");
    assert_eq!(
        started.verification_uri,
        "https://auth.openai.com/codex/device"
    );
    let finished = wait(&login, &started.id).await;
    assert_eq!(finished.state, "succeeded", "{finished:?}");
    assert!(login.signed_in());

    let stored: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&auth).unwrap()).unwrap();
    assert_eq!(stored["openai-codex"]["type"], "oauth");
    assert_eq!(stored["openai-codex"]["access"], "access");
    assert_eq!(stored["openai-codex"]["refresh"], "refresh");

    login.sign_out().await.unwrap();
    assert!(!login.signed_in());
}
