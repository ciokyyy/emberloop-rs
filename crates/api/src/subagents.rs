//! Persistent reviewer Sessions and per-submission delivery through the shared steer queue.
use crate::{routes, AppState};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use emberloop_core::{
    events::{EventKind, SessionEvent},
    id::generate_id,
    steer::{ReviewerReport, SteerEntry},
};
use serde::Deserialize;
use serde_json::{json, Value};
use store::{sessions::ReviewerAssignment, SessionMeta};
use utoipa::ToSchema;

type Error = (StatusCode, String);

#[derive(Deserialize, ToSchema)]
#[serde(
    tag = "action",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum SubagentRequest {
    Spawn {
        role: String,
        ticket_path: String,
        starting_point: String,
        #[serde(default)]
        text: String,
    },
    Steer {
        id: String,
        ticket_path: String,
        starting_point: String,
        text: String,
    },
    Stop {
        id: String,
    },
    Result {
        id: String,
    },
    Submit {
        status: String,
        #[serde(default)]
        report: String,
        #[serde(default)]
        summary: String,
    },
}

#[utoipa::path(post, path = "/api/sessions/{id}/subagents", request_body = SubagentRequest,
    responses((status = 200, description = "Reviewer action accepted", body = Object),
    (status = 400, description = "Invalid request"), (status = 404, description = "Session not found")))]
pub async fn action(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<SubagentRequest>,
) -> Result<Json<Value>, Error> {
    let caller = state
        .sessions
        .get(&id)
        .await
        .ok_or((StatusCode::NOT_FOUND, "Session not found".into()))?;
    let value = match request {
        SubagentRequest::Submit {
            status,
            report,
            summary,
        } => submit(&state, &caller, status, report, summary).await?,
        SubagentRequest::Spawn {
            role,
            ticket_path,
            starting_point,
            text,
        } => {
            if caller.reviewer.is_some() {
                return Err(bad("Reviewers cannot spawn reviewers"));
            }
            let _guard = state.subagent_gate.lock().await;
            let child = spawn(&state, &caller, &role, &ticket_path, &starting_point).await?;
            review(&state, &child, &ticket_path, &starting_point, &text).await?;
            json!({ "id": child.id, "role": role })
        }
        SubagentRequest::Steer {
            id,
            ticket_path,
            starting_point,
            text,
        } => {
            let _guard = state.subagent_gate.lock().await;
            let child = child_of(&state, &caller.id, &id).await?;
            review(&state, &child, &ticket_path, &starting_point, &text).await?;
            json!({ "id": id })
        }
        SubagentRequest::Stop { id } => {
            child_of(&state, &caller.id, &id).await?;
            routes::cancel_session(State(state.clone()), Path(id.clone())).await?;
            json!({ "id": id, "status": "stopped" })
        }
        SubagentRequest::Result { id } => {
            child_of(&state, &caller.id, &id).await?;
            let reports: Vec<_> = state
                .steers
                .queue(&caller.id)
                .entries()
                .await
                .into_iter()
                .filter(|entry| {
                    entry
                        .report
                        .as_ref()
                        .is_some_and(|report| report.session_id == id)
                })
                .collect();
            json!({ "id": id, "active": state.steers.queue(&id).is_turn_active(), "reports": reports })
        }
    };
    Ok(Json(value))
}

fn bad(text: impl Into<String>) -> Error {
    (StatusCode::BAD_REQUEST, text.into())
}
fn internal(error: impl std::fmt::Display) -> Error {
    (StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
}

async fn child_of(state: &AppState, parent: &str, id: &str) -> Result<SessionMeta, Error> {
    state
        .sessions
        .get(id)
        .await
        .filter(|child| {
            child
                .reviewer
                .as_ref()
                .is_some_and(|reviewer| reviewer.parent_id == parent)
        })
        .ok_or((
            StatusCode::NOT_FOUND,
            "Reviewer not found under this parent".into(),
        ))
}

async fn spawn(
    state: &AppState,
    parent: &SessionMeta,
    role: &str,
    ticket: &str,
    starting_point: &str,
) -> Result<SessionMeta, Error> {
    if !matches!(role, "spec" | "standards") {
        return Err(bad("role must be spec or standards"));
    }
    validate_assignment(state, parent, ticket, starting_point).await?;
    if let Some(child) = state
        .sessions
        .list(Some(&parent.project_id))
        .await
        .into_iter()
        .find(|child| {
            child
                .reviewer
                .as_ref()
                .is_some_and(|reviewer| reviewer.parent_id == parent.id && reviewer.role == role)
        })
    {
        return Ok(child);
    }
    let settings = state.settings.read().await;
    let defaults = if role == "spec" {
        settings.spec_reviewer.clone()
    } else {
        settings.standards_reviewer.clone()
    };
    drop(settings);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let mut meta = SessionMeta {
        id: generate_id("ses"),
        project_id: parent.project_id.clone(),
        created_at: now,
        updated_at: now,
        last_used_at: Some(now),
        title: if role == "spec" {
            "Spec reviewer"
        } else {
            "Standards reviewer"
        }
        .into(),
        reviewer: Some(ReviewerAssignment {
            parent_id: parent.id.clone(),
            role: role.into(),
            ticket_path: ticket.into(),
            starting_point: starting_point.into(),
        }),
        ..SessionMeta::default()
    };
    crate::session_setup::configure(
        state,
        &mut meta,
        Some(defaults.model),
        defaults.reasoning_effort,
    )
    .await?;
    let child = state.sessions.insert(meta).await.map_err(internal)?;
    let event = SessionEvent::new(
        &parent.id,
        0,
        EventKind::Notice,
        json!({
            "action": "subagent_created", "text": format!("{} Session opened.", child.title)
        }),
    );
    let stored = state
        .get_event_log(&parent.id)
        .await
        .map_err(internal)?
        .append(event)
        .await
        .map_err(internal)?;
    state.event_bus.publish(stored);
    Ok(child)
}

async fn validate_assignment(
    state: &AppState,
    session: &SessionMeta,
    ticket: &str,
    starting_point: &str,
) -> Result<(), Error> {
    if ticket.trim().is_empty()
        || starting_point.trim().is_empty()
        || starting_point.len() > 256
        || starting_point.contains(['\n', '\r'])
    {
        return Err(bad(
            "A ticket path and captured implementation starting point are required",
        ));
    }
    let project = state
        .projects
        .get(&session.project_id)
        .await
        .ok_or(bad("Project not found"))?;
    let root = tokio::fs::canonicalize(&project.root)
        .await
        .map_err(internal)?;
    let path = tokio::fs::canonicalize(root.join(ticket))
        .await
        .map_err(|_| bad("Ticket must be an existing file in the Project"))?;
    if !path.starts_with(&root) || !path.is_file() {
        return Err(bad("Ticket must be an existing file in the Project"));
    }
    Ok(())
}

async fn review(
    state: &AppState,
    child: &SessionMeta,
    ticket: &str,
    starting_point: &str,
    text: &str,
) -> Result<(), Error> {
    validate_assignment(state, child, ticket, starting_point).await?;
    let mut meta = state
        .sessions
        .get(&child.id)
        .await
        .ok_or(bad("Reviewer disappeared"))?;
    let assignment = meta.reviewer.as_mut().ok_or(bad("Not a reviewer"))?;
    if state.steers.queue(&child.id).is_turn_active()
        && (assignment.ticket_path != ticket || assignment.starting_point != starting_point)
    {
        return Err((
            StatusCode::CONFLICT,
            "Wait for this review before changing its ticket or starting point".into(),
        ));
    }
    assignment.ticket_path = ticket.into();
    assignment.starting_point = starting_point.into();
    state.sessions.save(&meta).await.map_err(internal)?;
    let prompt = format!("Review ticket: {ticket}\nImplementation starting point: {starting_point}\nInclude committed and uncommitted changes, including untracked files. The parent pauses edits during review.\n{text}\nSubmit your separate report with submit when finished. Keep this Session for follow-up reviews.");
    routes::prompt_session(
        State(state.clone()),
        Path(child.id.clone()),
        Json(routes::PromptRequest {
            text: prompt,
            images: Vec::new(),
        }),
    )
    .await?;
    Ok(())
}

async fn submit(
    state: &AppState,
    child: &SessionMeta,
    status: String,
    report: String,
    summary: String,
) -> Result<Value, Error> {
    let assignment = child
        .reviewer
        .as_ref()
        .ok_or(bad("Only reviewer Sessions can submit"))?;
    let status = status.trim();
    if !matches!(status, "done" | "blocked") {
        return Err(bad("status must be done or blocked"));
    }
    for (name, text, limit) in [("report", &report, 12000), ("summary", &summary, 300)] {
        let count = text.chars().count();
        if count > limit {
            return Err(bad(format!(
                "{name} is {count} characters; shorten it to {limit} characters or less"
            )));
        }
    }
    let entry = deliver(
        state,
        &assignment.parent_id,
        ReviewerReport {
            session_id: child.id.clone(),
            role: assignment.role.clone(),
            ticket_path: assignment.ticket_path.clone(),
            status: status.into(),
            report,
            summary: summary.split_whitespace().collect::<Vec<_>>().join(" "),
        },
    )
    .await?;
    Ok(json!({ "id": entry.id, "status": status }))
}

pub async fn notify_failure(
    state: &AppState,
    child: &SessionMeta,
    status: &str,
    message: &str,
) -> Result<(), Error> {
    let Some(assignment) = &child.reviewer else {
        return Ok(());
    };
    deliver(state, &assignment.parent_id, ReviewerReport {
        session_id: child.id.clone(), role: assignment.role.clone(), ticket_path: assignment.ticket_path.clone(),
        status: status.into(), summary: format!("{} reviewer {status}", assignment.role),
        report: format!("Tell the user: {message}\nOpen reviewer Session {} to change its model or prompt it to continue. No automatic retry.", child.id),
    }).await?;
    Ok(())
}

async fn deliver(
    state: &AppState,
    parent: &str,
    report: ReviewerReport,
) -> Result<SteerEntry, Error> {
    if state.sessions.get(parent).await.is_none() {
        return Err((StatusCode::NOT_FOUND, "Parent Session not found".into()));
    }
    let entry = state
        .steers
        .queue(parent)
        .enqueue_report(report)
        .await
        .map_err(internal)?;
    publish_report(state, parent, &entry).await?;
    wake_parent(state, parent)
        .await
        .unwrap_or_else(|error| tracing::warn!(%error, "report remains queued"));
    Ok(entry)
}

async fn publish_report(state: &AppState, parent: &str, entry: &SteerEntry) -> Result<(), Error> {
    let event = SessionEvent::new(
        parent,
        0,
        EventKind::SteerQueued,
        json!({
            "id": entry.id, "text": entry.text, "kind": "subagent", "report": entry.report, "imagesCount": 0
        }),
    );
    let stored = state
        .get_event_log(parent)
        .await
        .map_err(internal)?
        .append(event)
        .await
        .map_err(internal)?;
    state.event_bus.publish(stored);
    Ok(())
}

async fn wake_parent(state: &AppState, id: &str) -> Result<(), String> {
    let queue = state.steers.queue(id);
    if queue.is_turn_active() {
        return Ok(());
    }
    state.driver.connect(id).await?;
    let waker = routes::DriverWaker {
        driver: state.driver.as_ref(),
        session_id: id,
    };
    emberloop_core::steer::deliver_idle(&queue, &waker, &state.event_bus, id)
        .await
        .map(|_| ())
}

/// Restore queued report delivery without automatically retrying interrupted reviewer work.
/// Rebuild a missing feed event if a crash happened after queue persistence.
pub async fn recover(state: &AppState) -> Result<(), Error> {
    let parents: std::collections::HashSet<_> = state
        .sessions
        .list(None)
        .await
        .into_iter()
        .filter_map(|meta| meta.reviewer.map(|assignment| assignment.parent_id))
        .collect();
    for parent in parents {
        if state.sessions.get(&parent).await.is_none() {
            continue;
        }
        let log = state.get_event_log(&parent).await.map_err(internal)?;
        let events = log
            .kind_tail(&EventKind::SteerQueued, usize::MAX)
            .await
            .map_err(internal)?;
        for entry in state.steers.queue(&parent).entries().await {
            if entry.report.is_some() && !events.iter().any(|event| event.data["id"] == entry.id) {
                publish_report(state, &parent, &entry).await?;
            }
        }
        if state.steers.queue(&parent).pending_len().await > 0 {
            wake_parent(state, &parent)
                .await
                .unwrap_or_else(|error| tracing::warn!(%error, "report remains queued"));
        }
    }
    Ok(())
}

pub async fn validate_defaults(
    state: &AppState,
    settings: &crate::state::Settings,
) -> Result<(), Error> {
    let entries = state.models.entries().await;
    let current = state.settings.read().await.clone();
    for (defaults, previous) in [
        (&settings.spec_reviewer, &current.spec_reviewer),
        (&settings.standards_reviewer, &current.standards_reviewer),
    ] {
        if defaults.model == previous.model
            && defaults.reasoning_effort == previous.reasoning_effort
        {
            continue;
        }
        let entry = entries
            .iter()
            .find(|entry| entry.id == defaults.model)
            .ok_or_else(|| bad(format!("Unknown reviewer model: {}", defaults.model)))?;
        if let Some(effort) = &defaults.reasoning_effort {
            if !entry.efforts.contains(effort) {
                return Err(bad(format!(
                    "{} does not support reasoning effort {effort}",
                    defaults.model
                )));
            }
        }
    }
    Ok(())
}

pub fn watch_errors(state: AppState) -> tokio::task::JoinHandle<()> {
    let mut events = state.event_bus.subscribe();
    tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(event) if event.kind == EventKind::Error && event.seq > 0 => {
                    if let Some(child) = state.sessions.get(&event.session_id).await {
                        let message = event.data["message"].as_str().unwrap_or("Reviewer failed");
                        if let Err(error) = notify_failure(&state, &child, "error", message).await {
                            tracing::error!(?error, "could not notify parent of reviewer error");
                        }
                    }
                }
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(count)) => {
                    tracing::error!(count, "reviewer error watcher lagged");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            }
        }
    })
}
