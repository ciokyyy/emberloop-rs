use axum::extract::Query;
use axum::response::sse::{Event, KeepAlive, Sse};
use futures::Stream;
use serde::Deserialize;
use std::convert::Infallible;
use std::pin::Pin;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct EventsQuery {
    pub after: Option<u64>,
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
        if let Ok(replayed) = log.replay(query.after).await {
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
