use axum::extract::Query;
use axum::response::sse::{Event, KeepAlive, Sse};
use futures::Stream;
use serde::Deserialize;
use std::convert::Infallible;
use std::pin::Pin;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

use crate::state::AppState;

use axum::extract::{Path, State};
use axum::Json;
use emberloop_core::events::SessionEvent;

/// A page of history behind the tail window: the newest `limit` blocks (runs of chunks, whole tool calls, prompts) with
/// `seq < before`. The client asks for one page per click, so its cost never
/// depends on how long the Session has been running.
const PAGE_LIMIT: usize = 8;
const MAX_PAGE_LIMIT: usize = 100;

#[derive(Debug, Deserialize)]
pub struct EventsQuery {
    pub after: Option<u64>,
    /// First load asks for a tail window instead of the whole log.
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct HistoryQuery {
    pub before: Option<u64>,
    pub limit: Option<usize>,
}

pub async fn session_history_handler(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<HistoryQuery>,
) -> Json<Vec<SessionEvent>> {
    let limit = query.limit.unwrap_or(PAGE_LIMIT).min(MAX_PAGE_LIMIT);
    let events = match state.get_event_log(&session_id).await {
        Ok(log) => log.page(query.before.unwrap_or(u64::MAX), limit).await,
        Err(_) => return Json(Vec::new()),
    };
    Json(events.unwrap_or_default())
}

pub async fn session_events_handler(
    state: AppState,
    session_id: String,
    query: Query<EventsQuery>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>> + Send> {
    // Subscribe before the replay so an event landing in between is not lost;
    // live events at or below the replayed high-water mark are dropped as duplicates.
    let rx = state.event_bus.subscribe();
    let mut initial_events = Vec::new();
    let mut high_water = query.after;
    if let Ok(log) = state.get_event_log(&session_id).await {
        // `after=0` is "from the start", the same as no cursor: with a limit it is a first load.
        let replayed = match (query.after.filter(|after| *after > 0), query.limit) {
            (None, Some(limit)) => log.tail(limit.min(MAX_PAGE_LIMIT)).await,
            (cursor, _) => log.replay(cursor).await,
        };
        if let Ok(replayed) = replayed {
            for ev in replayed {
                high_water = high_water.max(Some(ev.seq));
                if let Ok(json) = serde_json::to_string(&ev) {
                    initial_events.push(Event::default().data(json));
                }
            }
        }
    }

    let initial_stream = tokio_stream::iter(initial_events.into_iter().map(Ok));

    let target_session = session_id.clone();
    let live_stream = BroadcastStream::new(rx).filter_map(move |res| match res {
        // Unnumbered events are stored and republished by the persister first.
        Ok(ev) if ev.session_id == target_session && Some(ev.seq) > high_water => {
            high_water = Some(ev.seq);
            serde_json::to_string(&ev)
                .ok()
                .map(|json| Ok(Event::default().data(json)))
        }
        _ => None,
    });

    let stream: Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>> =
        Box::pin(initial_stream.chain(live_stream));
    Sse::new(stream).keep_alive(KeepAlive::default())
}
