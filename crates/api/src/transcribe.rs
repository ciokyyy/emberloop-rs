//! `POST /api/transcribe`: raw audio in, Groq transcript out. The key is the Groq key Providers saves.
use crate::AppState;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{header::CONTENT_TYPE, HeaderMap, StatusCode};
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{json, Value};
use std::path::Path;

pub const GROQ_URL: &str = "https://api.groq.com/openai/v1/audio/transcriptions";
const GROQ_MODEL: &str = "whisper-large-v3-turbo";
/// Groq's upload cap; the browser checks the same limit before posting.
pub const MAX_AUDIO_BYTES: usize = 25 * 1024 * 1024;
const MISSING_KEY: &str = "Add a Groq key in Providers to use voice input.";

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/api/transcribe",
        post(transcribe).layer(DefaultBodyLimit::max(MAX_AUDIO_BYTES)),
    )
}

/// Raw audio in, `{"text"}` out. The key stays on the daemon.
async fn transcribe(
    State(state): State<AppState>,
    headers: HeaderMap,
    audio: Bytes,
) -> Result<Json<Value>, (StatusCode, String)> {
    let content_type = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let Some(format) = audio_format(content_type) else {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("Not an audio type: {content_type}"),
        ));
    };
    let Some((keys_file, url)) = state.groq.clone() else {
        return Err((
            StatusCode::CONFLICT,
            "This daemon cannot transcribe audio.".into(),
        ));
    };
    let key = groq_key(&keys_file).ok_or((StatusCode::CONFLICT, MISSING_KEY.to_owned()))?;
    let file =
        reqwest::multipart::Part::bytes(audio.to_vec()).file_name(format!("recording.{format}"));
    let form = reqwest::multipart::Form::new()
        .part("file", file)
        .text("model", GROQ_MODEL);
    let response = reqwest::Client::new()
        .post(url)
        .bearer_auth(key)
        .multipart(form)
        .send()
        .await
        .map_err(|error| (StatusCode::BAD_GATEWAY, error.to_string()))?;
    let status = response.status();
    if !status.is_success() {
        let detail = response.text().await.unwrap_or_default();
        return Err((
            StatusCode::BAD_GATEWAY,
            format!("Groq answered {status}: {detail}"),
        ));
    }
    let body: Value = response
        .json()
        .await
        .map_err(|error| (StatusCode::BAD_GATEWAY, error.to_string()))?;
    let text = body["text"]
        .as_str()
        .ok_or((StatusCode::BAD_GATEWAY, "Groq sent no text".to_owned()))?;
    Ok(Json(json!({ "text": text.trim() })))
}

/// Groq's file extension for a browser audio type; `None` for anything else.
fn audio_format(content_type: &str) -> Option<&'static str> {
    let essence = content_type.split(';').next()?.trim().to_ascii_lowercase();
    Some(match essence.as_str() {
        "audio/webm" | "video/webm" => "webm",
        "audio/ogg" | "application/ogg" => "ogg",
        "audio/mp4" | "audio/m4a" | "audio/x-m4a" | "video/mp4" => "m4a",
        "audio/mpeg" | "audio/mp3" => "mp3",
        "audio/wav" | "audio/x-wav" | "audio/wave" => "wav",
        "audio/flac" | "audio/x-flac" => "flac",
        _ => return None,
    })
}

/// The Groq key Providers saves under `groq` in keys.json.
fn groq_key(keys_file: &Path) -> Option<String> {
    std::fs::read_to_string(keys_file)
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .and_then(|keys| keys["groq"].as_str().map(|key| key.trim().to_owned()))
        .filter(|key| !key.is_empty())
}

#[cfg(test)]
mod tests {
    use crate::{build_router, AppState};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::routing::post;
    use axum::Router;
    use emberloop_core::events::EventBus;
    use std::sync::{Arc, Mutex};
    use store::{DataRoot, ProjectsStore, SessionsStore};
    use tempfile::{tempdir, TempDir};
    use tower::ServiceExt;

    /// Fake Groq: answers `status` with `body` and records the Authorization header it saw.
    async fn fake_groq(status: u16, body: &'static str) -> (String, Arc<Mutex<Option<String>>>) {
        let seen = Arc::new(Mutex::new(None));
        let captured = Arc::clone(&seen);
        let app = Router::new().route(
            "/openai/v1/audio/transcriptions",
            post(move |headers: axum::http::HeaderMap| {
                let captured = Arc::clone(&captured);
                async move {
                    *captured.lock().unwrap() = headers
                        .get("authorization")
                        .map(|value| value.to_str().unwrap().to_owned());
                    (StatusCode::from_u16(status).unwrap(), body)
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!(
            "http://{}/openai/v1/audio/transcriptions",
            listener.local_addr().unwrap()
        );
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (url, seen)
    }

    /// Router with a keys file holding `key` (none when `None`) and Groq at `url`.
    async fn app(key: Option<&str>, url: &str) -> (Router, TempDir) {
        let dir = tempdir().unwrap();
        let data_root = DataRoot::new(dir.path());
        let projects = ProjectsStore::load(data_root.projects_file())
            .await
            .unwrap();
        let sessions = SessionsStore::load(data_root.clone()).await.unwrap();
        let keys = dir.path().join("keys.json");
        if let Some(key) = key {
            std::fs::write(&keys, format!(r#"{{"groq":"{key}"}}"#)).unwrap();
        }
        let state = AppState::new(data_root, projects, sessions, Arc::new(EventBus::default()))
            .with_groq_keys(keys, url);
        (build_router(state), dir)
    }

    fn upload(mime: &str) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri("/api/transcribe")
            .header("content-type", mime)
            .body(Body::from(vec![0_u8; 16]))
            .unwrap()
    }

    #[tokio::test]
    async fn sends_audio_to_groq_with_the_saved_key_and_returns_the_text() {
        let (url, seen) = fake_groq(200, r#"{"text":"hello there"}"#).await;
        let (router, _dir) = app(Some("gsk-test"), &url).await;
        let response = router
            .oneshot(upload("audio/webm;codecs=opus"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["text"], "hello there");
        assert_eq!(seen.lock().unwrap().as_deref(), Some("Bearer gsk-test"));
    }

    #[tokio::test]
    async fn without_a_groq_key_answers_409_and_does_not_call_groq() {
        let (url, seen) = fake_groq(200, r#"{"text":"x"}"#).await;
        let (router, _dir) = app(None, &url).await;
        let response = router.oneshot(upload("audio/webm")).await.unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert!(seen.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn a_groq_error_answers_502() {
        let (url, _seen) = fake_groq(401, r#"{"error":"invalid key"}"#).await;
        let (router, _dir) = app(Some("gsk-bad"), &url).await;
        let response = router.oneshot(upload("audio/wav")).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    }

    #[tokio::test]
    async fn a_non_audio_body_answers_400() {
        let (url, seen) = fake_groq(200, r#"{"text":"x"}"#).await;
        let (router, _dir) = app(Some("gsk-test"), &url).await;
        let response = router.oneshot(upload("image/png")).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(seen.lock().unwrap().is_none());
    }
}
