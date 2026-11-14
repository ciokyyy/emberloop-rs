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
        #[serde(default)]
        tier: Option<String>,
        #[serde(default)]
        ticket_path: String,
        #[serde(default)]
        starting_point: String,
        #[serde(default)]
        text: String,
        #[serde(default)]
        title: Option<String>,
    },
    Steer {
        id: String,
        #[serde(default)]
        tier: Option<String>,
        #[serde(default)]
        ticket_path: String,
        #[serde(default)]
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
            tier,
            ticket_path,
            starting_point,
            text,
            title,
        } => {
            ensure_not_subagent(&caller)?;
            let _guard = state.subagent_gate.lock().await;
            handle_spawn(
                &state,
                &caller,
                SpawnInput {
                    role,
                    tier,
                    ticket_path,
                    starting_point,
                    text,
                    title,
                },
            )
            .await?
        }
        SubagentRequest::Steer {
            id,
            tier,
            ticket_path,
            starting_point,
            text,
        } => {
            ensure_not_subagent(&caller)?;
            let _guard = state.subagent_gate.lock().await;
            handle_steer(
                &state,
                &caller,
                SteerInput {
                    id,
                    tier,
                    ticket_path,
                    starting_point,
                    text,
                },
            )
            .await?
        }
        SubagentRequest::Stop { id } => {
            ensure_not_subagent(&caller)?;
            child_of(&state, &caller.id, &id).await?;
            routes::cancel_session(State(state.clone()), Path(id.clone())).await?;
            json!({ "id": id, "status": "stopped" })
        }
        SubagentRequest::Result { id } => {
            ensure_not_subagent(&caller)?;
            handle_result(&state, &caller, id).await?
        }
    };
    Ok(Json(value))
}

fn ensure_not_subagent(caller: &SessionMeta) -> Result<(), Error> {
    if caller.reviewer.is_some() {
        return Err(bad("Subagents cannot manage other subagents"));
    }
    Ok(())
}

fn reject_reviewer_fields(ticket: &str, starting_point: &str) -> Result<(), Error> {
    if !ticket.trim().is_empty() || !starting_point.trim().is_empty() {
        return Err(bad(
            "ticketPath and startingPoint are only valid for spec and standards reviewers",
        ));
    }
    Ok(())
}

pub(crate) async fn prompt_child_raw(
    state: &AppState,
    child_id: &str,
    text: String,
) -> Result<(), Error> {
    routes::prompt_session(
        State(state.clone()),
        Path(child_id.to_string()),
        Json(routes::PromptRequest {
            text,
            images: Vec::new(),
        }),
    )
    .await?;
    Ok(())
}

use crate::subagent_recovery::execute_subagent_prompt;
pub use crate::subagent_recovery::{mark_cancelled, watch_errors};

fn ladder_or_default(
    ladder: &crate::state::FallbackLadder,
    fallback_model: &str,
) -> Vec<crate::state::ModelRung> {
    if ladder.is_empty() {
        vec![crate::state::ModelRung {
            model: fallback_model.to_string(),
            reasoning_effort: None,
        }]
    } else {
        ladder.rungs().to_vec()
    }
}

fn primary_rung(
    ladder: &crate::state::FallbackLadder,
    fallback_model: &str,
) -> crate::state::ModelRung {
    ladder_or_default(ladder, fallback_model)
        .into_iter()
        .next()
        .unwrap_or_else(|| crate::state::ModelRung {
            model: fallback_model.to_string(),
            reasoning_effort: None,
        })
}

fn rung_index_for_meta(ladder: &[crate::state::ModelRung], meta: &SessionMeta) -> usize {
    let effort = meta.thought_level.as_deref().filter(|s| !s.is_empty());
    ladder
        .iter()
        .position(|r| r.model == meta.model && r.reasoning_effort.as_deref() == effort)
        .or_else(|| ladder.iter().position(|r| r.model == meta.model))
        .unwrap_or(0)
}

fn ladder_for_assignment(
    settings: &crate::state::Settings,
    assignment: &ReviewerAssignment,
) -> Vec<crate::state::ModelRung> {
    let ladder = match assignment.role.as_str() {
        "spec" => &settings.spec_reviewer,
        "standards" => &settings.standards_reviewer,
        _ => SubagentTier::from_stored(assignment.tier.as_deref()).ladder(&settings.subagent_tiers),
    };
    ladder_or_default(ladder, &settings.default_model)
}

async fn create_subagent_session(
    state: &AppState,
    parent: &SessionMeta,
    title: String,
    assignment: ReviewerAssignment,
    model: String,
    reasoning_effort: Option<String>,
) -> Result<SessionMeta, Error> {
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
        title,
        reviewer: Some(assignment),
        ..SessionMeta::default()
    };
    crate::session_setup::configure(state, &mut meta, Some(model), reasoning_effort).await?;
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

struct SpawnInput {
    role: String,
    tier: Option<String>,
    ticket_path: String,
    starting_point: String,
    text: String,
    title: Option<String>,
}

struct SteerInput {
    id: String,
    tier: Option<String>,
    ticket_path: String,
    starting_point: String,
    text: String,
}

const REVIEWER_TIER_ERROR: &str = "tier is not supported for spec or standards reviewers";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SubagentTier {
    Smol,
    Mid,
    Top,
}

impl SubagentTier {
    fn parse(raw: &str) -> Result<Self, Error> {
        match raw.trim() {
            "smol" => Ok(Self::Smol),
            "mid" => Ok(Self::Mid),
            "top" => Ok(Self::Top),
            other => Err(bad(format!(
                "tier must be smol, mid, or top (got `{other}`)"
            ))),
        }
    }

    fn from_stored(raw: Option<&str>) -> Self {
        raw.and_then(|value| Self::parse(value).ok())
            .unwrap_or(Self::Mid)
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Smol => "smol",
            Self::Mid => "mid",
            Self::Top => "top",
        }
    }

    fn ladder(self, tiers: &crate::state::SubagentTiers) -> &crate::state::FallbackLadder {
        match self {
            Self::Smol => &tiers.smol,
            Self::Mid => &tiers.mid,
            Self::Top => &tiers.top,
        }
    }
}

fn delegate_label(role: &str) -> &'static str {
    if role == "scout" {
        "Scout"
    } else {
        "Worker"
    }
}

async fn handle_spawn(
    state: &AppState,
    caller: &SessionMeta,
    input: SpawnInput,
) -> Result<Value, Error> {
    if emberloop_core::steer::is_reviewer_role(&input.role) {
        if input.tier.is_some() {
            return Err(bad(REVIEWER_TIER_ERROR));
        }
        let child = spawn_reviewer(
            state,
            caller,
            &input.role,
            &input.ticket_path,
            &input.starting_point,
        )
        .await?;
        review(
            state,
            &child,
            &input.ticket_path,
            &input.starting_point,
            &input.text,
        )
        .await?;
        return Ok(json!({ "id": child.id, "role": input.role }));
    }
    if matches!(input.role.as_str(), "worker" | "scout") {
        let role = input.role.clone();
        let (child, tier) = spawn_delegate(state, caller, input).await?;
        return Ok(json!({ "id": child.id, "role": role, "tier": tier }));
    }
    Err(bad("role must be spec, standards, worker, or scout"))
}

async fn handle_steer(
    state: &AppState,
    caller: &SessionMeta,
    input: SteerInput,
) -> Result<Value, Error> {
    let child = child_of(state, &caller.id, &input.id).await?;
    let is_reviewer = child.reviewer.as_ref().is_some_and(|a| a.is_reviewer());
    if is_reviewer {
        if input.tier.is_some() {
            return Err(bad(REVIEWER_TIER_ERROR));
        }
        review(
            state,
            &child,
            &input.ticket_path,
            &input.starting_point,
            &input.text,
        )
        .await?;
        return Ok(json!({ "id": input.id }));
    }
    steer_delegate(state, &child, input).await
}

async fn handle_result(state: &AppState, caller: &SessionMeta, id: String) -> Result<Value, Error> {
    child_of(state, &caller.id, &id).await?;
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
    Ok(json!({
        "id": id,
        "active": state.steers.queue(&id).is_turn_active(),
        "reports": reports
    }))
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
            "Subagent not found under this parent".into(),
        ))
}

async fn spawn_reviewer(
    state: &AppState,
    parent: &SessionMeta,
    role: &str,
    ticket: &str,
    starting_point: &str,
) -> Result<SessionMeta, Error> {
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
    let defaults = {
        let settings = state.settings.read().await;
        let ladder = if role == "spec" {
            &settings.spec_reviewer
        } else {
            &settings.standards_reviewer
        };
        primary_rung(ladder, &settings.default_model)
    };
    let title = if role == "spec" {
        "Spec reviewer"
    } else {
        "Standards reviewer"
    }
    .into();
    create_subagent_session(
        state,
        parent,
        title,
        ReviewerAssignment {
            parent_id: parent.id.clone(),
            role: role.into(),
            ticket_path: ticket.into(),
            starting_point: starting_point.into(),
            tier: None,
        },
        defaults.model,
        Some(defaults.reasoning_effort.unwrap_or_default()),
    )
    .await
}

async fn spawn_delegate(
    state: &AppState,
    parent: &SessionMeta,
    input: SpawnInput,
) -> Result<(SessionMeta, &'static str), Error> {
    reject_reviewer_fields(&input.ticket_path, &input.starting_point)?;
    if input.text.trim().is_empty() {
        return Err(bad(format!(
            "text is required for {} subagents",
            input.role
        )));
    }
    let tier = input
        .tier
        .as_deref()
        .map(SubagentTier::parse)
        .transpose()?
        .unwrap_or(SubagentTier::Mid);
    let (ladder, preset) = {
        let settings = state.settings.read().await;
        let raw = tier.ladder(&settings.subagent_tiers);
        let ladder = ladder_or_default(raw, &settings.default_model);
        let preset = primary_rung(raw, &settings.default_model);
        (ladder, preset)
    };
    let default_title = format!("{} ({})", delegate_label(&input.role), tier.as_str());
    let title = input
        .title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .unwrap_or(&default_title)
        .to_string();
    let child = create_subagent_session(
        state,
        parent,
        title,
        ReviewerAssignment {
            parent_id: parent.id.clone(),
            role: input.role,
            ticket_path: String::new(),
            starting_point: String::new(),
            tier: Some(tier.as_str().into()),
        },
        preset.model,
        Some(preset.reasoning_effort.unwrap_or_default()),
    )
    .await?;
    execute_subagent_prompt(state, &child.id, ladder, 0, input.text).await?;
    Ok((child, tier.as_str()))
}

async fn steer_delegate(
    state: &AppState,
    child: &SessionMeta,
    input: SteerInput,
) -> Result<Value, Error> {
    reject_reviewer_fields(&input.ticket_path, &input.starting_point)?;
    if input.text.trim().is_empty() {
        return Err(bad("text is required"));
    }
    let (switched_tier, active_meta, ladder, rung_index) =
        if let Some(raw_tier) = input.tier.as_deref() {
            let next_tier = SubagentTier::parse(raw_tier)?;
            let (ladder, preset) = {
                let settings = state.settings.read().await;
                let raw = next_tier.ladder(&settings.subagent_tiers);
                let ladder = ladder_or_default(raw, &settings.default_model);
                let preset = primary_rung(raw, &settings.default_model);
                (ladder, preset)
            };
            let mut meta = child.clone();
            let effort = Some(preset.reasoning_effort.unwrap_or_default());
            crate::session_setup::configure(state, &mut meta, Some(preset.model), effort).await?;
            if let Some(assignment) = meta.reviewer.as_mut() {
                let label = delegate_label(&assignment.role);
                if assignment
                    .tier
                    .as_deref()
                    .is_some_and(|old| meta.title == format!("{label} ({old})"))
                    || meta.title == label
                {
                    meta.title = format!("{label} ({})", next_tier.as_str());
                }
                assignment.tier = Some(next_tier.as_str().into());
            }
            state.sessions.save(&meta).await.map_err(internal)?;
            (Some(next_tier.as_str()), meta, ladder, 0)
        } else {
            let ladder = {
                let settings = state.settings.read().await;
                child
                    .reviewer
                    .as_ref()
                    .map(|a| ladder_for_assignment(&settings, a))
                    .unwrap_or_else(|| {
                        ladder_or_default(&settings.subagent_tiers.mid, &settings.default_model)
                    })
            };
            let idx = rung_index_for_meta(&ladder, child);
            (None, child.clone(), ladder, idx)
        };
    execute_subagent_prompt(state, &active_meta.id, ladder, rung_index, input.text).await?;
    Ok(match switched_tier {
        Some(tier) => json!({ "id": child.id, "tier": tier }),
        None => json!({ "id": child.id }),
    })
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
    let prompt = format!(
        "Review ticket: {ticket}\nImplementation starting point: {starting_point}\nInclude committed and uncommitted changes, including untracked files. The parent pauses edits during review.\n{text}\nSubmit your separate report with submit when finished. Keep this Session for follow-up reviews."
    );
    let ladder = {
        let settings = state.settings.read().await;
        let assignment = meta.reviewer.as_ref().ok_or(bad("Not a reviewer"))?;
        ladder_for_assignment(&settings, assignment)
    };
    let rung_index = rung_index_for_meta(&ladder, &meta);
    execute_subagent_prompt(state, &meta.id, ladder, rung_index, prompt).await
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
        .ok_or(bad("Only subagent Sessions can submit"))?;
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
    let (summary, report) = if assignment.is_reviewer() {
        (
            format!("{} reviewer {status}", assignment.role),
            format!(
                "Tell the user: {message}\nOpen reviewer Session {} to change its model or prompt it to continue. No automatic retry.",
                child.id
            ),
        )
    } else {
        (
            format!("{} {status}", assignment.role),
            format!(
                "Tell the user: {message}\nOpen subagent Session {} to change its model or prompt it to continue. No automatic retry.",
                child.id
            ),
        )
    };
    deliver(
        state,
        &assignment.parent_id,
        ReviewerReport {
            session_id: child.id.clone(),
            role: assignment.role.clone(),
            ticket_path: assignment.ticket_path.clone(),
            status: status.into(),
            summary,
            report,
        },
    )
    .await?;
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
    for (ladder, previous) in [
        (&settings.spec_reviewer, &current.spec_reviewer),
        (&settings.standards_reviewer, &current.standards_reviewer),
        (&settings.subagent_tiers.smol, &current.subagent_tiers.smol),
        (&settings.subagent_tiers.mid, &current.subagent_tiers.mid),
        (&settings.subagent_tiers.top, &current.subagent_tiers.top),
    ] {
        if ladder.is_empty() {
            return Err(bad("Fallback ladder must include at least one model"));
        }
        if ladder == previous {
            continue;
        }
        for rung in ladder.rungs() {
            if previous.rungs().contains(rung) {
                continue;
            }
            let entry = entries
                .iter()
                .find(|entry| entry.id == rung.model)
                .ok_or_else(|| bad(format!("Unknown subagent model: {}", rung.model)))?;
            if let Some(effort) = &rung.reasoning_effort {
                if !entry.efforts.contains(effort) {
                    return Err(bad(format!(
                        "{} does not support reasoning effort {effort}",
                        rung.model
                    )));
                }
            }
        }
    }
    Ok(())
}
