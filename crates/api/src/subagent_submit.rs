//! Submit enforcement: every subagent turn must end with an accepted `submit`.
use crate::state::AppState;
use emberloop_core::events::{EventKind, SessionEvent};
use emberloop_core::steer::SubagentReport;

pub(crate) const NUDGE_PROMPT: &str =
    "You ended without calling submit. Call submit(status, report, summary) now; do not do other work.";
const MAX_NUDGES: u32 = 3;
const REPORT_LIMIT: usize = 12000;

/// One subagent turn: how many nudges it took, whether a report was accepted, and its final text.
#[derive(Default)]
pub(crate) struct SubmitGuard {
    nudges: u32,
    submitted: bool,
    text: String,
}

pub(crate) async fn begin_turn(state: &AppState, child_id: &str) {
    state
        .subagent_submits
        .lock()
        .await
        .insert(child_id.to_string(), SubmitGuard::default());
}

/// Only a delivery that left `submit` is an accepted report; a refused one leaves the turn open.
pub(crate) async fn record_submit(state: &AppState, child_id: &str) {
    if let Some(guard) = state.subagent_submits.lock().await.get_mut(child_id) {
        guard.submitted = true;
    }
}

pub(crate) async fn forget(state: &AppState, child_id: &str) {
    state.subagent_submits.lock().await.remove(child_id);
}

pub fn watch_submits(state: AppState) -> tokio::task::JoinHandle<()> {
    let mut events = state.event_bus.subscribe();
    tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(event) if event.seq > 0 => match event.kind {
                    EventKind::AssistantChunk => remember_text(&state, &event).await,
                    EventKind::Status => match event.data["state"].as_str().unwrap_or("") {
                        "idle" => enforce(&state, &event.session_id).await,
                        "cancelled" => forget(&state, &event.session_id).await,
                        _ => {}
                    },
                    _ => {}
                },
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(count)) => {
                    tracing::error!(count, "subagent submit watcher lagged");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            }
        }
    })
}

async fn remember_text(state: &AppState, event: &SessionEvent) {
    let Some(text) = event.data["text"].as_str() else {
        return;
    };
    let mut guards = state.subagent_submits.lock().await;
    if let Some(guard) = guards.get_mut(&event.session_id) {
        guard.text.push_str(text);
        let total = guard.text.chars().count();
        if total > REPORT_LIMIT {
            guard.text = guard.text.chars().skip(total - REPORT_LIMIT).collect();
        }
    }
}

/// Next nudge number for this turn; `None` once an accepted report or no open turn is left.
async fn next_nudge(state: &AppState, child_id: &str) -> Option<u32> {
    let mut guards = state.subagent_submits.lock().await;
    let guard = guards.get_mut(child_id)?;
    if guard.submitted {
        guards.remove(child_id);
        return None;
    }
    guard.nudges += 1;
    Some(guard.nudges)
}

/// Whether the open turn already has an accepted report.
async fn is_submitted(state: &AppState, child_id: &str) -> bool {
    let guards = state.subagent_submits.lock().await;
    guards.get(child_id).is_some_and(|guard| guard.submitted)
}

/// Its report is delivered, so the stop is quiet: no "stopped" report to the parent.
async fn stop_finished_goldfish(state: &AppState, child_id: &str) {
    if let Err(error) = crate::routes::stop_session(state, child_id, false).await {
        tracing::warn!(?error, "could not stop a goldfish after its report");
    }
}

async fn nudge(state: &AppState, child_id: &str) {
    if let Err(error) =
        crate::subagents::prompt_child_raw(state, child_id, NUDGE_PROMPT.to_string()).await
    {
        tracing::warn!(?error, "could not nudge a subagent to submit");
    }
}

async fn enforce(state: &AppState, child_id: &str) {
    let Some(child) = state.sessions.get(child_id).await else {
        forget(state, child_id).await;
        return;
    };
    let Some(assignment) = child.reviewer.clone() else {
        return;
    };
    let submitted = is_submitted(state, child_id).await;
    match next_nudge(state, child_id).await {
        None if submitted && assignment.role == crate::subagents::GOLDFISH => {
            stop_finished_goldfish(state, child_id).await;
        }
        None => {}
        Some(nudges) if nudges <= MAX_NUDGES => nudge(state, child_id).await,
        Some(_) => file_blocked(state, &child, &assignment).await,
    }
}

/// The parent must never stay silent, so the bound ends in a report carrying the turn's own text.
async fn file_blocked(
    state: &AppState,
    child: &store::SessionMeta,
    assignment: &store::sessions::ReviewerAssignment,
) {
    let text = {
        let mut guards = state.subagent_submits.lock().await;
        guards
            .remove(&child.id)
            .map(|guard| guard.text)
            .unwrap_or_default()
    };
    let body = text.trim();
    let report = SubagentReport {
        session_id: child.id.clone(),
        role: assignment.role.clone(),
        ticket_path: assignment.ticket_path.clone(),
        status: "blocked".into(),
        summary: format!("{} ended without a report", assignment.role),
        report: if body.is_empty() {
            "The subagent ended its turn without calling submit and left no final text.".into()
        } else {
            body.to_string()
        },
    };
    if let Err(error) = crate::subagents::deliver(state, &assignment.parent_id, report).await {
        tracing::error!(
            ?error,
            "could not file a blocked report for a missing submit"
        );
    }
    if assignment.role == crate::subagents::GOLDFISH {
        stop_finished_goldfish(state, &child.id).await;
    }
}
