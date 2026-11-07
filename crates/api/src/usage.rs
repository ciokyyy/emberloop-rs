//! The Session's prompt-cache history: one record per `usage` event in its event log.
use axum::extract::{Path, Query, State};
use axum::Json;
use emberloop_core::events::{EventKind, SessionEvent};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use crate::state::AppState;

const DEFAULT_LIMIT: usize = 200;
const MAX_LIMIT: usize = 1000;

#[derive(Debug, Deserialize)]
pub struct UsageQuery {
    /// The newest this many requests; default 200, at most 1000.
    pub limit: Option<usize>,
}

/// One model request of a Session. `inputTokens` is the whole prompt (the context size);
/// `freshInputTokens` is the part that was neither a cache read nor a cache write.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UsageRecord {
    pub seq: u64,
    /// Epoch milliseconds when the event was recorded.
    pub at: u64,
    pub input_tokens: u64,
    pub fresh_input_tokens: u64,
    pub cached_tokens: u64,
    pub cache_creation_tokens: u64,
    pub output_tokens: u64,
    /// `cachedTokens` as a percentage of the prompt (`cached + creation + fresh`); null when the prompt is empty.
    pub hit_percent: Option<f64>,
}

/// The record a usage event stands for, or None for any other kind of event.
pub fn record_from(event: &SessionEvent) -> Option<UsageRecord> {
    if event.kind != EventKind::Usage {
        return None;
    }
    let count = |key: &str| event.data.get(key).and_then(Value::as_u64).unwrap_or(0);
    let input_tokens = count("inputTokens");
    let cached_tokens = count("cachedTokens");
    let cache_creation_tokens = count("cacheCreationTokens");
    let fresh_input_tokens = input_tokens
        .saturating_sub(cached_tokens)
        .saturating_sub(cache_creation_tokens);
    let prompt = cached_tokens + cache_creation_tokens + fresh_input_tokens;
    let hit_percent = (prompt > 0).then(|| cached_tokens as f64 / prompt as f64 * 100.0);
    Some(UsageRecord {
        seq: event.seq,
        at: event.at,
        input_tokens,
        fresh_input_tokens,
        cached_tokens,
        cache_creation_tokens,
        output_tokens: count("outputTokens"),
        hit_percent,
    })
}

#[utoipa::path(
    get,
    path = "/api/sessions/{id}/usage",
    params(
        ("id" = String, Path, description = "Session id"),
        ("limit" = Option<usize>, Query, description = "newest requests to return, default 200, at most 1000")
    ),
    responses((status = 200, description = "usage history, oldest first", body = Vec<UsageRecord>))
)]
pub async fn session_usage_handler(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<UsageQuery>,
) -> Json<Vec<UsageRecord>> {
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT);
    let Ok(log) = state.get_event_log(&session_id).await else {
        return Json(Vec::new());
    };
    let Ok(events) = log.replay(None).await else {
        return Json(Vec::new());
    };
    let mut records: Vec<UsageRecord> = events.iter().filter_map(record_from).collect();
    let start = records.len().saturating_sub(limit);
    Json(records.split_off(start))
}
