//! Subagent fallback ladder recovery engine: error classification, same-rung retries, and rung escalation.
use crate::state::AppState;
use axum::http::StatusCode;
use emberloop_core::events::{EventKind, SessionEvent};
use std::sync::Arc;
use tokio::sync::Notify;

type Error = (StatusCode, String);

pub(crate) const CONTINUATION_PROMPT: &str = "The previous model attempt was interrupted after partial progress. Continue the same task from the existing conversation history without repeating completed work.";

// Hard rules run before transient rules so a 429 quota/billing error escalates immediately instead of retrying.
const HARD_STATUS_CODES: &[&str] = &["401", "402", "403", "400"];
const HARD_ERROR_NEEDLES: &[&str] = &[
    "unauthorized",
    "forbidden",
    "authentication",
    "invalid api key",
    "invalid_api_key",
    "not signed in",
    "quota",
    "billing",
    "insufficient_quota",
    "payment required",
    "unknown model",
    "model not found",
    "model_not_found",
    "no such model",
    "unsupported model",
    "model not available",
    "is not available",
    "bad request",
    "invalid_request",
    "process exited",
    "failed to spawn",
    "spawn failed",
    "no stdin",
    "acp connection closed",
    "process error",
    "did not init",
    "did not create session",
    "failed to start",
    "crashed",
];

const TRANSIENT_STATUS_CODES: &[&str] = &["429", "500", "502", "503", "504", "529"];
const TRANSIENT_ERROR_NEEDLES: &[&str] = &[
    "rate limit",
    "rate_limit",
    "too many requests",
    "resource exhausted",
    "resource_exhausted",
    "overloaded",
    "internal server error",
    "bad gateway",
    "service unavailable",
    "gateway timeout",
    "timeout",
    "timed out",
    "deadline exceeded",
    "stream was interrupted",
    "unexpected eof",
    "mid-stream",
    "disconnect",
    "connection reset",
    "stream closed",
    "broken pipe",
];

#[derive(Clone, Debug)]
enum RunPhase {
    InSyncPrompt { sync_error: Option<String> },
    Running { errored_this_turn: bool },
    Recovering,
    Terminal,
}

#[derive(Clone, Debug)]
pub(crate) struct SubagentRun {
    generation: u64,
    ladder: Vec<crate::state::ModelRung>,
    rung_index: usize,
    transient_attempts: u32,
    task_prompt: String,
    had_progress: bool,
    phase: RunPhase,
    wake_cancel: Arc<Notify>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ErrorClass {
    Transient,
    Hard,
}

enum RecoveryStep {
    RetrySameRung {
        delay: std::time::Duration,
        prompt: String,
        wake_cancel: Arc<Notify>,
    },
    EscalateRung {
        rung: crate::state::ModelRung,
        prompt: String,
    },
}

enum RecoveryOutcome {
    Recovered,
    Aborted,
    Exhausted(String),
}

fn contains_code(lower: &str, code: &str) -> bool {
    let bytes = lower.as_bytes();
    for (idx, _) in lower.match_indices(code) {
        let left_ok = idx == 0 || !bytes[idx - 1].is_ascii_alphanumeric();
        let right = idx + code.len();
        let right_ok = right == bytes.len() || !bytes[right].is_ascii_alphanumeric();
        if left_ok && right_ok {
            return true;
        }
    }
    false
}

fn classify_error(message: &str) -> ErrorClass {
    let lower = message.to_ascii_lowercase();
    if HARD_STATUS_CODES
        .iter()
        .any(|code| contains_code(&lower, code))
        || HARD_ERROR_NEEDLES
            .iter()
            .any(|needle| lower.contains(needle))
    {
        return ErrorClass::Hard;
    }
    if TRANSIENT_STATUS_CODES
        .iter()
        .any(|code| contains_code(&lower, code))
        || TRANSIENT_ERROR_NEEDLES
            .iter()
            .any(|needle| lower.contains(needle))
    {
        return ErrorClass::Transient;
    }
    ErrorClass::Hard
}

fn prompt_for_recovery(run: &SubagentRun) -> String {
    if run.had_progress {
        CONTINUATION_PROMPT.to_string()
    } else {
        run.task_prompt.clone()
    }
}

pub async fn mark_cancelled(state: &AppState, session_id: &str) {
    let mut runs = state.subagent_runs.lock().await;
    if let Some(run) = runs.get_mut(session_id) {
        run.generation += 1;
        run.phase = RunPhase::Terminal;
        run.wake_cancel.notify_waiters();
    }
}

pub async fn remove_run(state: &AppState, session_id: &str) {
    let mut runs = state.subagent_runs.lock().await;
    if let Some(run) = runs.remove(session_id) {
        run.wake_cancel.notify_waiters();
    }
}

async fn finish_exhausted(state: &AppState, child_id: &str, final_error: &str) {
    if let Some(current) = state.sessions.get(child_id).await {
        if let Err(error) =
            crate::subagents::notify_failure(state, &current, "error", final_error).await
        {
            tracing::error!(?error, "could not notify parent of subagent error");
        }
    }
}

pub(crate) async fn execute_subagent_prompt(
    state: &AppState,
    child_id: &str,
    ladder: Vec<crate::state::ModelRung>,
    rung_index: usize,
    prompt: String,
) -> Result<(), Error> {
    let generation = {
        let mut runs = state.subagent_runs.lock().await;
        let next_gen = runs.get(child_id).map_or(1, |prev| {
            prev.wake_cancel.notify_waiters();
            prev.generation + 1
        });
        runs.insert(
            child_id.to_string(),
            SubagentRun {
                generation: next_gen,
                ladder,
                rung_index,
                transient_attempts: 0,
                task_prompt: prompt.clone(),
                had_progress: false,
                phase: RunPhase::InSyncPrompt { sync_error: None },
                wake_cancel: Arc::new(Notify::new()),
            },
        );
        next_gen
    };

    let send_res = crate::subagents::prompt_child_raw(state, child_id, prompt).await;
    let initial_error = {
        let mut runs = state.subagent_runs.lock().await;
        let Some(run) = runs.get_mut(child_id) else {
            return Ok(());
        };
        if run.generation != generation {
            return Ok(());
        }
        let sync_error = match &mut run.phase {
            RunPhase::InSyncPrompt { sync_error } => sync_error.take(),
            RunPhase::Terminal | RunPhase::Running { .. } | RunPhase::Recovering => return Ok(()),
        };
        match (sync_error, send_res) {
            (Some(err), _) | (None, Err((_, err))) => {
                run.phase = RunPhase::Recovering;
                err
            }
            (None, Ok(())) => {
                run.phase = RunPhase::Running {
                    errored_this_turn: false,
                };
                return Ok(());
            }
        }
    };

    match recover_loop(state, child_id, generation, initial_error).await {
        RecoveryOutcome::Recovered | RecoveryOutcome::Aborted => Ok(()),
        RecoveryOutcome::Exhausted(final_error) => {
            finish_exhausted(state, child_id, &final_error).await;
            Err((StatusCode::INTERNAL_SERVER_ERROR, final_error))
        }
    }
}

async fn reconfigure_for_rung(
    state: &AppState,
    child_id: &str,
    rung: &crate::state::ModelRung,
) -> Result<(), Error> {
    let mut meta = state
        .sessions
        .get(child_id)
        .await
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "Subagent disappeared".into()))?;
    let effort = Some(rung.reasoning_effort.clone().unwrap_or_default());
    let configured =
        crate::session_setup::configure(state, &mut meta, Some(rung.model.clone()), effort.clone())
            .await;
    if let Err((StatusCode::CONFLICT, _)) = &configured {
        // If configure_model just evicted a dead harness handle on rollback failure, retry once on fresh spawn state.
        if !state.steers.queue(child_id).is_turn_active() {
            crate::session_setup::configure(state, &mut meta, Some(rung.model.clone()), effort)
                .await?;
        } else {
            configured?;
        }
    } else {
        configured?;
    }
    state
        .sessions
        .save(&meta)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(())
}

async fn next_recovery_step(
    state: &AppState,
    child_id: &str,
    generation: u64,
    current_error: &str,
) -> Result<RecoveryStep, RecoveryOutcome> {
    let mut runs = state.subagent_runs.lock().await;
    let Some(run) = runs.get_mut(child_id) else {
        return Err(RecoveryOutcome::Exhausted(current_error.to_string()));
    };
    if run.generation != generation || matches!(run.phase, RunPhase::Terminal) {
        return Err(RecoveryOutcome::Aborted);
    }
    if classify_error(current_error) == ErrorClass::Transient && run.transient_attempts < 2 {
        let delay = state.subagent_retry_backoffs[run.transient_attempts as usize];
        run.transient_attempts += 1;
        return Ok(RecoveryStep::RetrySameRung {
            delay,
            prompt: prompt_for_recovery(run),
            wake_cancel: Arc::clone(&run.wake_cancel),
        });
    }
    if run.rung_index + 1 < run.ladder.len() {
        run.rung_index += 1;
        run.transient_attempts = 0;
        return Ok(RecoveryStep::EscalateRung {
            rung: run.ladder[run.rung_index].clone(),
            prompt: prompt_for_recovery(run),
        });
    }
    run.phase = RunPhase::Terminal;
    Err(RecoveryOutcome::Exhausted(current_error.to_string()))
}

#[allow(clippy::disallowed_methods)] // prod 2s/5s transient retry backoff; tests pass Duration::ZERO
async fn wait_retry_backoff(delay: std::time::Duration, wake_cancel: &Notify) -> bool {
    if delay.is_zero() {
        return true;
    }
    tokio::select! {
        () = tokio::time::sleep(delay) => true,
        () = wake_cancel.notified() => false,
    }
}

async fn send_recovery_prompt(
    state: &AppState,
    child_id: &str,
    generation: u64,
    prompt: String,
) -> Result<RecoveryOutcome, String> {
    // Let watch_errors drain any EventKind::Error emitted before the previous attempt returned.
    tokio::task::yield_now().await;
    {
        let mut runs = state.subagent_runs.lock().await;
        let Some(run) = runs.get_mut(child_id) else {
            return Ok(RecoveryOutcome::Aborted);
        };
        if run.generation != generation || matches!(run.phase, RunPhase::Terminal) {
            return Ok(RecoveryOutcome::Aborted);
        }
        run.phase = RunPhase::InSyncPrompt { sync_error: None };
    }
    let send_res = crate::subagents::prompt_child_raw(state, child_id, prompt).await;
    let mut runs = state.subagent_runs.lock().await;
    let Some(run) = runs.get_mut(child_id) else {
        return Ok(RecoveryOutcome::Aborted);
    };
    if run.generation != generation {
        return Ok(RecoveryOutcome::Aborted);
    }
    let sync_error = match &mut run.phase {
        RunPhase::InSyncPrompt { sync_error } => sync_error.take(),
        RunPhase::Terminal => return Ok(RecoveryOutcome::Aborted),
        RunPhase::Running { .. } | RunPhase::Recovering => None,
    };
    match (sync_error, send_res) {
        (Some(err), _) | (None, Err((_, err))) => {
            run.phase = RunPhase::Recovering;
            Err(err)
        }
        (None, Ok(())) => {
            run.transient_attempts = 0;
            run.phase = RunPhase::Running {
                errored_this_turn: false,
            };
            Ok(RecoveryOutcome::Recovered)
        }
    }
}

async fn recover_loop(
    state: &AppState,
    child_id: &str,
    generation: u64,
    initial_error: String,
) -> RecoveryOutcome {
    let mut current_error = initial_error;
    loop {
        let step = match next_recovery_step(state, child_id, generation, &current_error).await {
            Ok(step) => step,
            Err(outcome) => return outcome,
        };
        let prompt = match step {
            RecoveryStep::RetrySameRung {
                delay,
                prompt,
                wake_cancel,
            } => {
                if !wait_retry_backoff(delay, &wake_cancel).await {
                    return RecoveryOutcome::Aborted;
                }
                prompt
            }
            RecoveryStep::EscalateRung { rung, prompt } => {
                if let Err((_, err)) = reconfigure_for_rung(state, child_id, &rung).await {
                    current_error = err;
                    continue;
                }
                prompt
            }
        };
        match send_recovery_prompt(state, child_id, generation, prompt).await {
            Ok(outcome) => return outcome,
            Err(err) => current_error = err,
        }
    }
}

async fn handle_status_event(state: &AppState, event: &SessionEvent) {
    match event.data["state"].as_str().unwrap_or("") {
        "cancelled" => mark_cancelled(state, &event.session_id).await,
        "working" => {
            let mut runs = state.subagent_runs.lock().await;
            if let Some(SubagentRun {
                phase: RunPhase::Running { errored_this_turn },
                ..
            }) = runs.get_mut(&event.session_id)
            {
                *errored_this_turn = false;
            }
        }
        "idle" => {
            let mut runs = state.subagent_runs.lock().await;
            if let Some(SubagentRun {
                had_progress,
                phase: RunPhase::Running { errored_this_turn },
                ..
            }) = runs.get_mut(&event.session_id)
            {
                if !*errored_this_turn {
                    *had_progress = true;
                }
            }
        }
        _ => {}
    }
}

async fn handle_progress_event(state: &AppState, session_id: &str) {
    let mut runs = state.subagent_runs.lock().await;
    if let Some(run) = runs.get_mut(session_id) {
        if !matches!(run.phase, RunPhase::Terminal) {
            run.had_progress = true;
        }
    }
}

async fn handle_error_event(state: &AppState, event: &SessionEvent) {
    let message = event.data["message"]
        .as_str()
        .unwrap_or("Subagent failed")
        .to_string();
    let spawn_gen = {
        let mut runs = state.subagent_runs.lock().await;
        match runs.get_mut(&event.session_id) {
            None => None,
            Some(run) => match &mut run.phase {
                RunPhase::Terminal | RunPhase::Recovering => return,
                RunPhase::InSyncPrompt { sync_error } => {
                    *sync_error = Some(message);
                    return;
                }
                RunPhase::Running { .. } => {
                    run.phase = RunPhase::Recovering;
                    Some(run.generation)
                }
            },
        }
    };
    let Some(generation) = spawn_gen else {
        let Some(child) = state.sessions.get(&event.session_id).await else {
            return;
        };
        if child.reviewer.is_some() {
            finish_exhausted(state, &child.id, &message).await;
        }
        return;
    };
    let state_clone = state.clone();
    let child_id = event.session_id.clone();
    tokio::spawn(async move {
        if let RecoveryOutcome::Exhausted(final_error) =
            recover_loop(&state_clone, &child_id, generation, message).await
        {
            finish_exhausted(&state_clone, &child_id, &final_error).await;
        }
    });
}

pub fn watch_errors(state: AppState) -> tokio::task::JoinHandle<()> {
    let mut events = state.event_bus.subscribe();
    tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(event) if event.seq > 0 => match event.kind {
                    EventKind::Status => handle_status_event(&state, &event).await,
                    EventKind::AssistantChunk | EventKind::ToolFinished => {
                        handle_progress_event(&state, &event.session_id).await
                    }
                    EventKind::Error => handle_error_event(&state, &event).await,
                    _ => {}
                },
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(count)) => {
                    tracing::error!(count, "subagent error watcher lagged");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::{classify_error, ErrorClass};

    #[test]
    fn classifies_transient_errors() {
        for msg in [
            "429 Too Many Requests",
            "upstream rate_limit reached",
            "500 Internal Server Error",
            "502 Bad Gateway",
            "503 Service Unavailable",
            "504 Gateway Timeout",
            "529 Overloaded",
            "request timed out",
            "stream was interrupted mid-stream",
            "unexpected EOF while reading SSE",
            "connection reset by peer",
        ] {
            assert_eq!(classify_error(msg), ErrorClass::Transient, "for: {msg}");
        }
    }

    #[test]
    fn classifies_hard_errors_including_quota_on_429_and_unknown_default() {
        for msg in [
            "401 Unauthorized",
            "402 Payment Required",
            "403 Forbidden",
            "400 Bad Request",
            "429 insufficient_quota: check billing",
            "quota exceeded for project",
            "unknown model: foo-9",
            "model not found",
            "model not available on this account",
            "failed to spawn harness binary",
            "acp connection closed",
            "provider failed",
        ] {
            assert_eq!(classify_error(msg), ErrorClass::Hard, "for: {msg}");
        }
    }

    #[test]
    fn status_codes_require_non_alphanumeric_boundaries() {
        // Numeric substrings inside identifiers or larger numbers do not trigger status code rules.
        assert_eq!(
            classify_error("timeout connecting on port 5000"),
            ErrorClass::Transient
        );
        assert_eq!(
            classify_error("rate limit for user 4012"),
            ErrorClass::Transient
        );
    }
}
