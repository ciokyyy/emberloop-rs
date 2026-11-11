//! Web Push: the alert while the app is closed. The page owns the in-app notice and its sound, so
//! the daemon sends a push only when no page reported that Session visible within [`GRACE`], which
//! wakes exactly one client instead of two.
use crate::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use ct_codecs::{Base64UrlSafeNoPadding, Encoder};
use emberloop_core::events::{EventKind, SessionEvent};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use web_push::{
    request_builder::build_request, ContentEncoding, SubscriptionInfo, VapidSignatureBuilder,
    WebPushError, WebPushMessageBuilder,
};

/// How long a page's heartbeat stands for that page being open.
pub const GRACE: Duration = Duration::from_secs(30);
const VAPID_FILE: &str = "push-vapid.json";
const SUBSCRIPTIONS_FILE: &str = "push-subscriptions.json";
/// The VAPID contact; Chrome refuses a signed request that names none.
const CONTACT: &str = "mailto:emberloop@localhost";
const PUSH_TIMEOUT: Duration = Duration::from_secs(10);

/// One browser's push subscription, exactly as `pushManager.subscribe` reports it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Subscription {
    pub endpoint: String,
    pub p256dh: String,
    pub auth: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct VapidKeys {
    private: String,
    public: String,
}

/// Subscriptions and visibility stamps. A browser subscribes once; the daemon keeps the keypair.
pub struct Push {
    root: PathBuf,
    keys: VapidKeys,
    subscriptions: Mutex<Vec<Subscription>>,
    visible: Mutex<HashMap<String, Instant>>,
    http: reqwest::Client,
}

impl Push {
    /// Reads the keypair and the subscriptions from the data root, creating the keypair once.
    pub fn load(root: &Path) -> Push {
        let keys = read_keys(root).unwrap_or_else(|error| {
            tracing::warn!(%error, "push keypair unreadable; generating one");
            generate_keys(root)
        });
        Push {
            root: root.to_path_buf(),
            keys,
            subscriptions: Mutex::new(read_subscriptions(root)),
            visible: Mutex::new(HashMap::new()),
            http: reqwest::Client::builder()
                .timeout(PUSH_TIMEOUT)
                .build()
                .unwrap_or_default(),
        }
    }

    /// The base64url public key the browser passes to `pushManager.subscribe`.
    pub fn public_key(&self) -> &str {
        &self.keys.public
    }

    /// A page reported this Session on screen just now.
    pub fn seen(&self, session_id: &str) {
        self.seen_at(session_id, Instant::now());
    }

    fn seen_at(&self, session_id: &str, now: Instant) {
        if let Ok(mut visible) = self.visible.lock() {
            visible.insert(session_id.to_string(), now);
        }
    }

    /// No page held this Session within [`GRACE`], so an alert has to come from the push service.
    pub fn hidden(&self, session_id: &str) -> bool {
        self.hidden_at(session_id, Instant::now())
    }

    fn hidden_at(&self, session_id: &str, now: Instant) -> bool {
        let Ok(visible) = self.visible.lock() else {
            return true;
        };
        visible
            .get(session_id)
            .is_none_or(|seen| now.duration_since(*seen) >= GRACE)
    }

    /// Stores a browser's subscription once; the same endpoint replaces the one held.
    pub fn subscribe(&self, subscription: Subscription) {
        if let Ok(mut subscriptions) = self.subscriptions.lock() {
            subscriptions.retain(|held| held.endpoint != subscription.endpoint);
            subscriptions.push(subscription);
            write_subscriptions(&self.root, &subscriptions);
        }
    }

    pub fn unsubscribe(&self, endpoint: &str) {
        if let Ok(mut subscriptions) = self.subscriptions.lock() {
            subscriptions.retain(|held| held.endpoint != endpoint);
            write_subscriptions(&self.root, &subscriptions);
        }
    }

    pub fn subscribers(&self) -> usize {
        self.subscriptions
            .lock()
            .map(|held| held.len())
            .unwrap_or(0)
    }

    /// Sends `{title, body, url}` to every subscribed browser, and forgets the endpoints the push
    /// service reports as gone.
    pub async fn send(&self, title: &str, body: &str, url: &str) {
        let payload = serde_json::json!({ "title": title, "body": body, "url": url }).to_string();
        let held = self
            .subscriptions
            .lock()
            .map(|subscriptions| subscriptions.clone())
            .unwrap_or_default();
        for subscription in held {
            self.deliver(&subscription, payload.as_bytes()).await;
        }
    }

    /// One message to one browser: a gone endpoint is forgotten here, so `send` stays a flat loop.
    async fn deliver(&self, subscription: &Subscription, payload: &[u8]) {
        match self.send_one(subscription, payload).await {
            Ok(()) => {}
            Err(Outcome::Gone) => self.forget(subscription),
            Err(Outcome::Failed(error)) => tracing::warn!(%error, "push failed"),
        }
    }

    fn forget(&self, subscription: &Subscription) {
        tracing::info!(endpoint = %subscription.endpoint, "push subscription is gone");
        self.unsubscribe(&subscription.endpoint);
    }

    async fn send_one(&self, subscription: &Subscription, payload: &[u8]) -> Result<(), Outcome> {
        let info = SubscriptionInfo::new(
            subscription.endpoint.clone(),
            subscription.p256dh.clone(),
            subscription.auth.clone(),
        );
        let failed = |error: WebPushError| Outcome::Failed(error.to_string());
        let mut signature = VapidSignatureBuilder::from_base64_no_sub(&self.keys.private)
            .map_err(failed)?
            .add_sub_info(&info);
        signature.add_claim("sub", CONTACT);
        let mut builder = WebPushMessageBuilder::new(&info);
        builder.set_payload(ContentEncoding::Aes128Gcm, payload);
        builder.set_ttl(600);
        builder.set_vapid_signature(signature.build().map_err(failed)?);
        let request = build_request::<Vec<u8>>(builder.build().map_err(failed)?);
        let mut outgoing = self
            .http
            .request(reqwest::Method::POST, request.uri().to_string());
        for (name, value) in request.headers() {
            outgoing = outgoing.header(name.as_str(), value.as_bytes());
        }
        let response = outgoing
            .body(request.into_body())
            .send()
            .await
            .map_err(|error| Outcome::Failed(error.to_string()))?;
        if matches!(response.status().as_u16(), 404 | 410) {
            return Err(Outcome::Gone);
        }
        if !response.status().is_success() {
            return Err(Outcome::Failed(format!(
                "push service {}",
                response.status()
            )));
        }
        Ok(())
    }
}

enum Outcome {
    /// The push service forgot this browser: drop the subscription.
    Gone,
    Failed(String),
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/push/key", get(public_key))
        .route("/api/push/subscribe", post(subscribe))
        .route("/api/push/unsubscribe", post(unsubscribe))
        .route("/api/push/visible", post(visible))
        .route("/api/push/test", post(test))
}

async fn public_key(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "key": state.push.public_key() }))
}

async fn subscribe(
    State(state): State<AppState>,
    Json(subscription): Json<Subscription>,
) -> StatusCode {
    if subscription.endpoint.is_empty()
        || subscription.p256dh.is_empty()
        || subscription.auth.is_empty()
    {
        return StatusCode::BAD_REQUEST;
    }
    state.push.subscribe(subscription);
    StatusCode::NO_CONTENT
}

#[derive(Debug, Deserialize)]
struct Endpoint {
    endpoint: String,
}

async fn unsubscribe(State(state): State<AppState>, Json(body): Json<Endpoint>) -> StatusCode {
    state.push.unsubscribe(&body.endpoint);
    StatusCode::NO_CONTENT
}

#[derive(Debug, Deserialize)]
pub struct Seen {
    #[serde(rename = "sessionId")]
    pub session_id: String,
}

async fn visible(State(state): State<AppState>, Json(body): Json<Seen>) -> StatusCode {
    state.push.seen(&body.session_id);
    StatusCode::NO_CONTENT
}

async fn test(State(state): State<AppState>) -> StatusCode {
    state
        .push
        .send("Emberloop", "Notifications work.", "/")
        .await;
    StatusCode::NO_CONTENT
}

fn read_keys(root: &Path) -> Result<VapidKeys, String> {
    let text = std::fs::read_to_string(root.join(VAPID_FILE)).map_err(|error| error.to_string())?;
    serde_json::from_str(&text).map_err(|error| error.to_string())
}

fn generate_keys(root: &Path) -> VapidKeys {
    let pair = jwt_simple::algorithms::ES256KeyPair::generate();
    let Ok(private) = Base64UrlSafeNoPadding::encode_to_string(pair.to_bytes()) else {
        return VapidKeys::default();
    };
    let public = VapidSignatureBuilder::from_base64_no_sub(&private)
        .ok()
        .and_then(|builder| Base64UrlSafeNoPadding::encode_to_string(builder.get_public_key()).ok())
        .unwrap_or_default();
    let keys = VapidKeys { private, public };
    if let Ok(text) = serde_json::to_string(&keys) {
        if let Err(error) = std::fs::write(root.join(VAPID_FILE), text) {
            tracing::warn!(%error, "push keypair not saved");
        }
    }
    keys
}

fn read_subscriptions(root: &Path) -> Vec<Subscription> {
    std::fs::read_to_string(root.join(SUBSCRIPTIONS_FILE))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn write_subscriptions(root: &Path, subscriptions: &[Subscription]) {
    let Ok(text) = serde_json::to_string(subscriptions) else {
        return;
    };
    if let Err(error) = std::fs::write(root.join(SUBSCRIPTIONS_FILE), text) {
        tracing::warn!(%error, "push subscriptions not saved");
    }
}

/// Publishes one alert per finished turn or error, unless a page holds that Session.
pub fn watch(state: AppState) {
    let mut events = state.event_bus.subscribe();
    tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(event) => notify(&state, &event).await,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
    });
}

async fn notify(state: &AppState, event: &SessionEvent) {
    let Some(body) = alert(event) else {
        return;
    };
    if !state.push.hidden(&event.session_id) {
        return;
    }
    let Some(meta) = state.sessions.get(&event.session_id).await else {
        return;
    };
    let title = if meta.title.is_empty() {
        "Emberloop".to_string()
    } else {
        meta.title.clone()
    };
    let url = format!("/p/{}/s/{}", meta.project_id, event.session_id);
    state.push.send(&title, &body, &url).await;
}

/// What the page would have shown: a finished turn or an error, and nothing else.
fn alert(event: &SessionEvent) -> Option<String> {
    match &event.kind {
        EventKind::Status if event.data["state"] == "idle" => Some("Turn finished".to_string()),
        EventKind::Error => {
            let message = event.data["message"].as_str().unwrap_or_default();
            Some(message.chars().take(140).collect())
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ct_codecs::Decoder;

    fn push() -> (tempfile::TempDir, Push) {
        let dir = tempfile::tempdir().unwrap();
        let push = Push::load(dir.path());
        (dir, push)
    }

    #[test]
    fn a_page_seen_within_the_grace_window_suppresses_the_push() {
        let (_dir, push) = push();
        let now = Instant::now();
        push.seen_at("session", now);
        assert!(!push.hidden_at("session", now + Duration::from_secs(10)));
        assert!(push.hidden_at("session", now + GRACE));
        assert!(push.hidden_at("other", now));
    }

    #[test]
    fn the_keypair_survives_a_restart_and_names_a_usable_public_key() {
        let (dir, first) = push();
        let public = first.public_key().to_string();
        let bytes = Base64UrlSafeNoPadding::decode_to_vec(&public, None).unwrap();
        assert_eq!(bytes.len(), 65);
        assert_eq!(Push::load(dir.path()).public_key(), public);
    }

    #[test]
    fn a_subscription_is_stored_once_and_removed_by_its_endpoint() {
        let (dir, push) = push();
        let one = Subscription {
            endpoint: "https://push.example/1".into(),
            p256dh: "key".into(),
            auth: "secret".into(),
        };
        push.subscribe(one.clone());
        push.subscribe(one.clone());
        assert_eq!(push.subscribers(), 1);
        assert_eq!(Push::load(dir.path()).subscribers(), 1);
        push.unsubscribe(&one.endpoint);
        assert_eq!(push.subscribers(), 0);
        assert_eq!(Push::load(dir.path()).subscribers(), 0);
    }

    #[test]
    fn only_a_finished_turn_or_an_error_is_an_alert() {
        let event = |kind: EventKind, data: serde_json::Value| SessionEvent {
            v: 1,
            seq: 1,
            session_id: "session".into(),
            kind,
            at: 0,
            data,
        };
        assert!(alert(&event(
            EventKind::Status,
            serde_json::json!({"state": "idle"})
        ))
        .is_some());
        assert!(alert(&event(
            EventKind::Status,
            serde_json::json!({"state": "working"})
        ))
        .is_none());
        assert!(alert(&event(
            EventKind::Error,
            serde_json::json!({"message": "boom"})
        ))
        .is_some());
        assert!(alert(&event(EventKind::Notice, serde_json::json!({}))).is_none());
    }
}
