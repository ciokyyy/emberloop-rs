#![allow(clippy::unwrap_used, clippy::expect_used)]
use std::time::Duration;

use emberloop_core::events::{EventBus, EventKind, SessionEvent};

fn unnumbered() -> SessionEvent {
    SessionEvent::new("ses_1", 0, EventKind::ThinkingChunk, serde_json::json!({}))
}

#[tokio::test]
async fn settled_waits_for_every_unnumbered_event_to_be_stored() {
    let bus = EventBus::default();
    bus.persister_started();
    bus.publish(unnumbered());
    bus.publish(unnumbered());
    assert!(!bus.settled(Duration::from_millis(20)).await);
    bus.event_stored();
    assert!(!bus.settled(Duration::from_millis(20)).await);
    bus.event_stored();
    assert!(bus.settled(Duration::from_millis(20)).await);
}

#[tokio::test]
async fn settled_wakes_when_the_last_event_is_stored() {
    let bus = std::sync::Arc::new(EventBus::default());
    bus.persister_started();
    bus.publish(unnumbered());
    let waiter = tokio::spawn({
        let bus = std::sync::Arc::clone(&bus);
        async move { bus.settled(Duration::from_secs(30)).await }
    });
    tokio::task::yield_now().await;
    bus.event_stored();
    assert!(waiter.await.unwrap());
}

#[tokio::test]
async fn without_a_persister_nothing_is_waited_for() {
    let bus = EventBus::default();
    bus.publish(unnumbered());
    assert!(bus.settled(Duration::from_secs(30)).await);
}

#[tokio::test]
async fn numbered_events_are_never_counted() {
    let bus = EventBus::default();
    bus.persister_started();
    bus.publish(SessionEvent::new(
        "ses_1",
        7,
        EventKind::AssistantChunk,
        serde_json::json!({}),
    ));
    assert!(bus.settled(Duration::from_secs(30)).await);
}

#[tokio::test]
async fn a_lagged_persister_forgets_what_it_missed() {
    let bus = EventBus::default();
    bus.persister_started();
    bus.publish(unnumbered());
    bus.events_lost();
    assert!(bus.settled(Duration::from_secs(30)).await);
}
