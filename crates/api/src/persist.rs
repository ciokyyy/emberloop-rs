use emberloop_core::events::SessionEvent;
use tokio::sync::broadcast::error::RecvError;

use crate::state::AppState;

/// Stores every event published without a log number (the proxy's usage,
/// steer deliveries, runner errors), then republishes the stored copy so the
/// live stream and a later replay carry the same numbered event.
pub fn spawn_persister(state: AppState) -> tokio::task::JoinHandle<()> {
    let mut events = state.event_bus.subscribe();
    tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(event) if event.seq == 0 => persist(&state, event).await,
                Ok(_) | Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => return,
            }
        }
    })
}

async fn persist(state: &AppState, event: SessionEvent) {
    if state.sessions.get(&event.session_id).await.is_none() {
        return;
    }
    let Ok(log) = state.get_event_log(&event.session_id).await else {
        return;
    };
    if let Ok(stored) = log.append(event).await {
        state.event_bus.publish(stored);
    }
}
