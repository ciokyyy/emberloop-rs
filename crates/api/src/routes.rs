use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use emberloop_core::events::{EventKind, SessionEvent};
use emberloop_core::id::generate_id;
use prompts::PromptInfo;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use store::{Project, SessionMeta};
use utoipa::{OpenApi, ToSchema};

use crate::models::ModelEntry;
use crate::sse::{session_events_handler, EventsQuery};
use crate::state::{AppState, Settings};

#[derive(OpenApi)]
#[openapi(
    paths(
        list_projects,
        create_project,
        update_project,
        delete_project,
        list_sessions,
        create_session,
        update_session,
        delete_session,
        prompt_session,
        steer_session,
        cancel_session,
        get_settings,
        update_settings,
        list_tool_keys,
        set_tool_key,
        list_prompts,
        update_prompt,
        delete_prompt,
        get_prompt,
        list_models,
        refresh_models,
        crate::providers::list_providers,
        crate::providers::provider_usage,
        crate::usage::session_usage_handler,
        crate::providers::start_agy_login,
        crate::providers::get_agy_login,
        crate::providers::cancel_agy_login,
        crate::providers::complete_agy_login,
        crate::providers::sign_out_agy_account,
        crate::providers::set_provider_key,
        crate::providers::delete_provider_key,
        crate::providers::start_codex_login,
        crate::providers::get_codex_login,
        crate::providers::cancel_codex_login,
        crate::providers::start_claude_login,
        crate::providers::get_claude_login,
        crate::providers::cancel_claude_login,
        crate::providers::complete_claude_login,
    ),
    components(
        schemas(
            ProjectDto,
            CreateProjectRequest,
            UpdateProjectRequest,
            SessionDto,
            CreateSessionRequest,
            UpdateSessionRequest,
            PromptRequest,
            PromptImageDto,
            SteerRequest,
            Settings,
            ToolKeyDto,
            ToolKeyRequest,
            PromptDto,
            UpdatePromptRequest,
            PromptTextDto,
            ModelEntry,
            crate::providers::ProviderView,
            crate::usage::UsageRecord,
            crate::providers::UsageView,
            crate::providers::UsageWindow,
            crate::providers::UsageBalance,
            crate::providers::AccountView,
            crate::providers::LoginView,
            crate::providers::LoginRequest,
            crate::providers::CompleteLoginRequest,
            crate::providers::CompleteClaudeLoginRequest,
            crate::providers::SetKeyRequest,
        )
    ),
    tags(
        (name = "emberloop", description = "emberloop-rs REST API")
    )
)]
pub struct ApiDoc;

#[derive(Serialize, Deserialize, ToSchema)]
pub struct ProjectDto {
    pub id: String,
    pub name: String,
    pub root: String,
}

#[derive(Deserialize, ToSchema)]
pub struct CreateProjectRequest {
    pub name: String,
    pub root: String,
}

#[derive(Deserialize, ToSchema)]
pub struct UpdateProjectRequest {
    pub name: Option<String>,
    pub root: Option<String>,
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct SessionDto {
    pub id: String,
    #[serde(rename = "projectId")]
    pub project_id: String,
    pub title: String,
    #[serde(rename = "ponytailLevel")]
    pub ponytail_level: String,
    pub model: String,
    #[serde(rename = "thoughtLevel", skip_serializing_if = "Option::is_none")]
    pub thought_level: Option<String>,
    #[serde(rename = "createdAt")]
    pub created_at: u64,
    #[serde(rename = "updatedAt")]
    pub updated_at: u64,
}

#[derive(Deserialize, ToSchema)]
pub struct CreateSessionRequest {
    #[serde(rename = "projectId")]
    pub project_id: String,
    pub title: Option<String>,
    #[serde(rename = "ponytailLevel")]
    pub ponytail_level: Option<String>,
    pub model: Option<String>,
    #[serde(rename = "thoughtLevel")]
    pub thought_level: Option<String>,
}

#[derive(Deserialize, ToSchema)]
pub struct UpdateSessionRequest {
    pub title: Option<String>,
    pub model: Option<String>,
    #[serde(rename = "thoughtLevel")]
    pub thought_level: Option<String>,
}

#[derive(Deserialize, ToSchema)]
pub struct PromptImageDto {
    #[serde(rename = "mimeType")]
    pub mime_type: String,
    #[serde(rename = "dataBase64")]
    pub data_base64: String,
}

#[derive(Deserialize, ToSchema)]
pub struct PromptRequest {
    pub text: String,
    #[serde(default)]
    pub images: Vec<PromptImageDto>,
}

#[derive(Deserialize, ToSchema)]
pub struct SteerRequest {
    pub text: String,
}

#[derive(Deserialize)]
pub struct ListSessionsQuery {
    #[serde(rename = "projectId")]
    pub project_id: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/projects",
    responses(
        (status = 200, description = "List all projects", body = Vec<ProjectDto>)
    )
)]
pub async fn list_projects(State(state): State<AppState>) -> Json<Vec<ProjectDto>> {
    let list = state.projects.list().await;
    let dtos = list
        .into_iter()
        .map(|p| ProjectDto {
            id: p.id,
            name: p.name,
            root: p.root.to_string_lossy().to_string(),
        })
        .collect();
    Json(dtos)
}

#[utoipa::path(
    post,
    path = "/api/projects",
    request_body = CreateProjectRequest,
    responses(
        (status = 201, description = "Project created", body = ProjectDto),
        (status = 500, description = "Storage error")
    )
)]
pub async fn create_project(
    State(state): State<AppState>,
    Json(payload): Json<CreateProjectRequest>,
) -> Result<(StatusCode, Json<ProjectDto>), (StatusCode, String)> {
    let project = Project {
        id: generate_id("proj"),
        name: payload.name,
        root: PathBuf::from(payload.root),
    };
    let saved = state
        .projects
        .insert(project)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok((
        StatusCode::CREATED,
        Json(ProjectDto {
            id: saved.id,
            name: saved.name,
            root: saved.root.to_string_lossy().to_string(),
        }),
    ))
}

#[utoipa::path(
    patch,
    path = "/api/projects/{id}",
    request_body = UpdateProjectRequest,
    responses(
        (status = 200, description = "Project updated", body = ProjectDto),
        (status = 404, description = "Project not found")
    )
)]
pub async fn update_project(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(payload): Json<UpdateProjectRequest>,
) -> Result<Json<ProjectDto>, (StatusCode, String)> {
    let root = payload.root.map(PathBuf::from);
    let updated = state
        .projects
        .update(&id, payload.name, root)
        .await
        .map_err(|e| (StatusCode::NOT_FOUND, e.to_string()))?;
    Ok(Json(ProjectDto {
        id: updated.id,
        name: updated.name,
        root: updated.root.to_string_lossy().to_string(),
    }))
}

#[utoipa::path(
    delete,
    path = "/api/projects/{id}",
    responses(
        (status = 200, description = "Project deleted", body = ProjectDto),
        (status = 404, description = "Project not found")
    )
)]
pub async fn delete_project(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ProjectDto>, (StatusCode, String)> {
    let deleted = state
        .projects
        .delete(&id)
        .await
        .map_err(|e| (StatusCode::NOT_FOUND, e.to_string()))?;
    Ok(Json(ProjectDto {
        id: deleted.id,
        name: deleted.name,
        root: deleted.root.to_string_lossy().to_string(),
    }))
}

#[utoipa::path(
    get,
    path = "/api/sessions",
    responses(
        (status = 200, description = "List sessions", body = Vec<SessionDto>)
    )
)]
pub async fn list_sessions(
    State(state): State<AppState>,
    Query(query): Query<ListSessionsQuery>,
) -> Json<Vec<SessionDto>> {
    let list = state.sessions.list(query.project_id.as_deref()).await;
    let dtos = list.into_iter().map(session_dto).collect();
    Json(dtos)
}

#[utoipa::path(
    post,
    path = "/api/sessions",
    request_body = CreateSessionRequest,
    responses(
        (status = 201, description = "Session created", body = SessionDto),
        (status = 404, description = "Project not found")
    )
)]
pub async fn create_session(
    State(state): State<AppState>,
    Json(payload): Json<CreateSessionRequest>,
) -> Result<(StatusCode, Json<SessionDto>), (StatusCode, String)> {
    if state.projects.get(&payload.project_id).await.is_none() {
        return Err((StatusCode::NOT_FOUND, "Project not found".into()));
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    let default_level = {
        let settings = state.settings.read().await;
        settings.ponytail_default_level.clone()
    };

    let meta = SessionMeta {
        id: generate_id("ses"),
        project_id: payload.project_id,
        title: payload.title.unwrap_or_else(|| "New Session".into()),
        ponytail_level: payload.ponytail_level.unwrap_or(default_level),
        model: payload
            .model
            .unwrap_or_else(|| "gemini-3.8-flash".to_string()),
        thought_level: payload.thought_level.filter(|l| !l.trim().is_empty()),
        created_at: now,
        updated_at: now,
        last_used_at: Some(now),
        prompt_hashes: HashMap::new(),
        ..SessionMeta::default()
    };

    let saved = state
        .sessions
        .insert(meta)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok((StatusCode::CREATED, Json(session_dto(saved))))
}

#[utoipa::path(
    patch,
    path = "/api/sessions/{id}",
    request_body = UpdateSessionRequest,
    responses(
        (status = 200, description = "Session updated", body = SessionDto),
        (status = 404, description = "Session not found")
    )
)]
pub async fn update_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(payload): Json<UpdateSessionRequest>,
) -> Result<Json<SessionDto>, (StatusCode, String)> {
    let mut updated = state
        .sessions
        .get(&id)
        .await
        .ok_or_else(|| (StatusCode::NOT_FOUND, format!("Session not found: {id}")))?;
    if payload.model.is_some() || payload.thought_level.is_some() {
        crate::session_setup::configure(&state, &mut updated, payload.model, payload.thought_level)
            .await?;
    }
    updated.updated_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    state
        .sessions
        .save(&updated)
        .await
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    if let Some(title) = payload.title {
        updated = state
            .sessions
            .update_title(&id, &title)
            .await
            .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    } else if let Some(current) = state.sessions.get(&id).await {
        updated = current;
    }
    Ok(Json(session_dto(updated)))
}

#[utoipa::path(
    delete,
    path = "/api/sessions/{id}",
    responses(
        (status = 200, description = "Session deleted", body = SessionDto),
        (status = 404, description = "Session not found")
    )
)]
pub async fn delete_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<SessionDto>, (StatusCode, String)> {
    let deleted = state
        .sessions
        .delete(&id)
        .await
        .map_err(|e| (StatusCode::NOT_FOUND, e.to_string()))?;

    Ok(Json(session_dto(deleted)))
}

fn session_dto(s: SessionMeta) -> SessionDto {
    SessionDto {
        id: s.id,
        project_id: s.project_id,
        title: s.title,
        ponytail_level: s.ponytail_level,
        model: s.model,
        thought_level: s.thought_level,
        created_at: s.created_at,
        updated_at: s.updated_at,
    }
}

#[utoipa::path(
    post,
    path = "/api/sessions/{id}/prompt",
    request_body = PromptRequest,
    responses(
        (status = 200, description = "Prompt submitted"),
        (status = 404, description = "Session not found")
    )
)]
pub async fn prompt_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(payload): Json<PromptRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    let meta = state
        .sessions
        .get(&id)
        .await
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Session not found".into()))?;
    crate::session_setup::validate_images(&state, &meta, &payload.images).await?;
    // A turn already running takes the message as a steer. The UI's own idea of
    // `working` is stale after a page reload; the queue's is not.
    if state.steers.queue(&id).is_turn_active() {
        let steer = Json(SteerRequest { text: payload.text });
        return steer_session(State(state), Path(id), steer).await;
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    state
        .sessions
        .touch(&id, now)
        .await
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;

    let log = state
        .get_event_log(&id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let event = SessionEvent::new(
        &id,
        0,
        EventKind::Prompt,
        serde_json::json!({
            "text": payload.text,
            "imagesCount": payload.images.len()
        }),
    );
    let appended = log
        .append(event)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    state.event_bus.publish(appended);

    let status_event = SessionEvent::new(
        &id,
        0,
        EventKind::Status,
        serde_json::json!({ "state": "working" }),
    );
    if let Ok(appended_status) = log.append(status_event).await {
        state.event_bus.publish(appended_status);
    }
    let images = payload
        .images
        .into_iter()
        .map(|image| crate::routes::PromptImageDto {
            mime_type: image.mime_type,
            data_base64: image.data_base64,
        })
        .collect();
    state
        .driver
        .prompt(&id, &payload.text, images)
        .await
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error))?;

    Ok(StatusCode::OK)
}

#[utoipa::path(
    post,
    path = "/api/sessions/{id}/steer",
    request_body = SteerRequest,
    responses(
        (status = 200, description = "Steer queued"),
        (status = 404, description = "Session not found")
    )
)]
pub async fn steer_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(payload): Json<SteerRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    if state.sessions.get(&id).await.is_none() {
        return Err((StatusCode::NOT_FOUND, "Session not found".into()));
    }

    let log = state
        .get_event_log(&id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let queue = state.steers.queue(&id);
    let entry = queue
        .enqueue(emberloop_core::steer::SteerKind::Steer, payload.text)
        .await;
    let event = SessionEvent::new(
        &id,
        0,
        EventKind::SteerQueued,
        serde_json::json!({
            "id": entry.id,
            "text": entry.text,
            "kind": "steer"
        }),
    );
    let appended = log
        .append(event)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    state.event_bus.publish(appended);

    // A Session with no turn running takes the idle route; a busy one keeps the
    // entry pending for the proxy to inject at its next request.
    let _ = state.driver.connect(&id).await;
    let waker = DriverWaker {
        driver: state.driver.as_ref(),
        session_id: &id,
    };
    let _ = emberloop_core::steer::deliver_idle(&queue, &waker, &state.event_bus, &id).await;

    Ok(StatusCode::OK)
}

#[utoipa::path(
    post,
    path = "/api/sessions/{id}/cancel",
    responses(
        (status = 200, description = "Session cancellation requested"),
        (status = 404, description = "Session not found")
    )
)]
pub async fn cancel_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    if state.sessions.get(&id).await.is_none() {
        return Err((StatusCode::NOT_FOUND, "Session not found".into()));
    }

    let log = state
        .get_event_log(&id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let event = SessionEvent::new(
        &id,
        0,
        EventKind::Status,
        serde_json::json!({"state": "cancelled"}),
    );
    let appended = log
        .append(event)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    state.event_bus.publish(appended);
    state
        .driver
        .cancel(&id)
        .await
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error))?;
    Ok(StatusCode::OK)
}

#[utoipa::path(
    get,
    path = "/api/settings",
    responses(
        (status = 200, description = "Current settings", body = Settings)
    )
)]
pub async fn get_settings(State(state): State<AppState>) -> Json<Settings> {
    let settings = state.settings.read().await;
    Json(settings.clone())
}

#[utoipa::path(
    put,
    path = "/api/settings",
    request_body = Settings,
    responses(
        (status = 200, description = "Settings updated", body = Settings),
        (status = 400, description = "Invalid title model")
    )
)]
pub async fn update_settings(
    State(state): State<AppState>,
    Json(payload): Json<Settings>,
) -> Result<Json<Settings>, (StatusCode, String)> {
    if payload.title_model.len() > 256
        || payload.title_model.chars().any(char::is_whitespace)
        || !payload
            .title_model
            .split_once('/')
            .is_some_and(|(provider, model)| !provider.is_empty() && !model.is_empty())
    {
        return Err((
            StatusCode::BAD_REQUEST,
            "Title model must be provider/model".into(),
        ));
    }
    let mut settings = state.settings.write().await;
    *settings = payload.clone();
    Ok(Json(payload))
}

#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ToolKeyDto {
    /// Settings key name: `brave`, `exa` or `jina`.
    pub name: String,
    /// The stored key name the tools read.
    pub variable: String,
    pub set: bool,
}

#[derive(Deserialize, ToSchema)]
pub struct ToolKeyRequest {
    pub value: String,
}

fn tool_key_dto(name: &str, variable: &str, set: bool) -> ToolKeyDto {
    ToolKeyDto {
        name: name.to_string(),
        variable: variable.to_string(),
        set,
    }
}

fn tool_keys_file_of(state: &AppState) -> Result<PathBuf, (StatusCode, String)> {
    state.tool_keys_file.clone().ok_or((
        StatusCode::CONFLICT,
        "This daemon has no config root, so tool keys cannot be saved.".to_string(),
    ))
}

#[utoipa::path(
    get,
    path = "/api/tool-keys",
    responses(
        (status = 200, description = "Tool keys and whether each is set", body = [ToolKeyDto]),
        (status = 409, description = "No config root")
    )
)]
pub async fn list_tool_keys(
    State(state): State<AppState>,
) -> Result<Json<Vec<ToolKeyDto>>, (StatusCode, String)> {
    let path = tool_keys_file_of(&state)?;
    let stored = store::tool_keys::read(&path)
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    let keys = store::tool_keys::NAMES
        .iter()
        .map(|(name, variable)| tool_key_dto(name, variable, stored.contains_key(*variable)))
        .collect();
    Ok(Json(keys))
}

#[utoipa::path(
    put,
    path = "/api/tool-keys/{name}",
    request_body = ToolKeyRequest,
    params(("name" = String, Path, description = "brave, exa or jina")),
    responses(
        (status = 200, description = "Key saved; the next tool call uses it", body = ToolKeyDto),
        (status = 400, description = "Value too long"),
        (status = 404, description = "Unknown tool key"),
        (status = 409, description = "No config root")
    )
)]
pub async fn set_tool_key(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(payload): Json<ToolKeyRequest>,
) -> Result<Json<ToolKeyDto>, (StatusCode, String)> {
    let path = tool_keys_file_of(&state)?;
    let Some(variable) = store::tool_keys::variable(&name) else {
        return Err((StatusCode::NOT_FOUND, format!("Unknown tool key {name}")));
    };
    let value = payload.value.trim();
    if value.len() > 4096 {
        return Err((StatusCode::BAD_REQUEST, "Key is too long".into()));
    }
    store::tool_keys::set(&path, variable, value)
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    Ok(Json(tool_key_dto(&name, variable, !value.is_empty())))
}

#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PromptDto {
    pub id: String,
    pub default_hash: String,
    pub override_hash: Option<String>,
    pub active_hash: String,
    pub differs: bool,
    pub error: Option<String>,
}

#[derive(Deserialize, ToSchema)]
pub struct UpdatePromptRequest {
    pub text: String,
}

#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PromptTextDto {
    pub id: String,
    pub text: String,
}

fn prompt_dto(info: PromptInfo) -> PromptDto {
    PromptDto {
        id: info.id,
        default_hash: info.default_hash,
        override_hash: info.override_hash,
        active_hash: info.active_hash,
        differs: info.differs,
        error: info.error,
    }
}

fn prompt_error(error: prompts::PromptError) -> (StatusCode, String) {
    let status = match error {
        prompts::PromptError::NotFound { .. } => StatusCode::NOT_FOUND,
        _ => StatusCode::BAD_REQUEST,
    };
    (status, error.to_string())
}

#[utoipa::path(
    get,
    path = "/api/prompts",
    responses(
        (status = 200, description = "Prompt ids with their hashes", body = Vec<PromptDto>)
    )
)]
pub async fn list_prompts(State(state): State<AppState>) -> Json<Vec<PromptDto>> {
    Json(state.prompts.list().into_iter().map(prompt_dto).collect())
}

#[utoipa::path(
    get,
    path = "/api/prompts/{id}",
    responses(
        (status = 200, description = "Active text of one prompt", body = PromptTextDto),
        (status = 404, description = "Unknown prompt id")
    )
)]
pub async fn get_prompt(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<PromptTextDto>, (StatusCode, String)> {
    let text = state
        .prompts
        .text(&id)
        .ok_or((StatusCode::NOT_FOUND, format!("prompt not found: {id}")))?;
    Ok(Json(PromptTextDto { id, text }))
}

#[utoipa::path(
    put,
    path = "/api/prompts/{id}",
    request_body = UpdatePromptRequest,
    responses(
        (status = 200, description = "Override written", body = PromptDto),
        (status = 400, description = "The override text is invalid"),
        (status = 404, description = "Unknown prompt id")
    )
)]
pub async fn update_prompt(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(payload): Json<UpdatePromptRequest>,
) -> Result<Json<PromptDto>, (StatusCode, String)> {
    state
        .prompts
        .set_override(&id, &payload.text)
        .map_err(prompt_error)?;
    let info = state
        .prompts
        .list()
        .into_iter()
        .find(|info| info.id == id)
        .ok_or((StatusCode::NOT_FOUND, format!("prompt not found: {id}")))?;
    Ok(Json(prompt_dto(info)))
}

#[utoipa::path(
    delete,
    path = "/api/prompts/{id}",
    responses(
        (status = 200, description = "Override removed", body = PromptDto),
        (status = 404, description = "Unknown prompt id")
    )
)]
pub async fn delete_prompt(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<PromptDto>, (StatusCode, String)> {
    state.prompts.remove_override(&id).map_err(prompt_error)?;
    let info = state
        .prompts
        .list()
        .into_iter()
        .find(|info| info.id == id)
        .ok_or((StatusCode::NOT_FOUND, format!("prompt not found: {id}")))?;
    Ok(Json(prompt_dto(info)))
}

#[utoipa::path(
    get,
    path = "/api/models",
    responses(
        (status = 200, description = "List known models", body = Vec<ModelEntry>)
    )
)]
pub async fn list_models(State(state): State<AppState>) -> Json<Vec<ModelEntry>> {
    Json(state.models.entries().await)
}

#[utoipa::path(
    post,
    path = "/api/models/refresh",
    responses(
        (status = 200, description = "Refreshed model list", body = Vec<ModelEntry>),
        (status = 500, description = "Catalogue persistence error")
    )
)]
pub async fn refresh_models(
    State(state): State<AppState>,
) -> Result<Json<Vec<ModelEntry>>, (StatusCode, String)> {
    state
        .models
        .refresh()
        .await
        .map(Json)
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))
}

pub fn build_router(state: AppState) -> Router {
    let providers = crate::providers::router();
    Router::new()
        .route("/api/projects", get(list_projects).post(create_project))
        .route(
            "/api/projects/{id}",
            patch(update_project).delete(delete_project),
        )
        .route("/api/sessions", get(list_sessions).post(create_session))
        .route(
            "/api/sessions/{id}",
            patch(update_session).delete(delete_session),
        )
        .route(
            "/api/sessions/{id}/prompt",
            post(prompt_session).layer(axum::extract::DefaultBodyLimit::max(70 * 1024 * 1024)),
        )
        .route("/api/sessions/{id}/steer", post(steer_session))
        .route("/api/sessions/{id}/cancel", post(cancel_session))
        .route(
            "/api/sessions/{id}/events",
            get(
                |state: State<AppState>, path: Path<String>, query: Query<EventsQuery>| {
                    session_events_handler(state.0, path.0, query)
                },
            ),
        )
        .route(
            "/api/sessions/{id}/history",
            get(crate::sse::session_history_handler),
        )
        .route(
            "/api/sessions/{id}/usage",
            get(crate::usage::session_usage_handler),
        )
        .route(
            "/api/version",
            get(|| async {
                Json(serde_json::json!({
                    "version": option_env!("EMBERLOOP_BUILD_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"))
                }))
            }),
        )
        .route("/api/settings", get(get_settings).put(update_settings))
        .route("/api/tool-keys", get(list_tool_keys))
        .route("/api/tool-keys/{name}", axum::routing::put(set_tool_key))
        .route("/api/models", get(list_models))
        .route("/api/models/refresh", post(refresh_models))
        .route("/api/prompts", get(list_prompts))
        .route(
            "/api/prompts/{id}",
            get(get_prompt).put(update_prompt).delete(delete_prompt),
        )
        .route("/openapi.json", get(|| async { Json(ApiDoc::openapi()) }))
        .merge(providers)
        .with_state(state)
}

struct DriverWaker<'a> {
    driver: &'a dyn crate::driver::SessionDriver,
    session_id: &'a str,
}

#[async_trait::async_trait]
impl emberloop_core::steer::SteerWaker for DriverWaker<'_> {
    async fn wake(&self, text: &str) -> Result<(), String> {
        self.driver.wake(self.session_id, text).await
    }
}

#[cfg(test)]
mod prompt_routes_tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use emberloop_core::events::EventBus;
    use std::sync::Arc;
    use store::{DataRoot, ProjectsStore, SessionsStore};
    use tempfile::tempdir;
    use tower::ServiceExt;

    async fn app() -> Router {
        let dir = tempdir().unwrap();
        let data_root = DataRoot::new(dir.path());
        let projects = ProjectsStore::load(data_root.projects_file())
            .await
            .unwrap();
        let sessions = SessionsStore::load(data_root.clone()).await.unwrap();
        std::mem::forget(dir);
        build_router(AppState::new(
            data_root,
            projects,
            sessions,
            Arc::new(EventBus::default()),
        ))
    }
    async fn json(response: axum::response::Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn version_reports_the_build_version() {
        let response = app()
            .await
            .oneshot(
                Request::builder()
                    .uri("/api/version")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            json(response).await["version"],
            option_env!("EMBERLOOP_BUILD_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"))
        );
    }

    #[tokio::test]
    async fn tool_keys_are_listed_written_and_cleared() {
        let dir = tempfile::tempdir().unwrap();
        let data_root = DataRoot::new(dir.path());
        let keys_file = dir.path().join("config/tool-keys.json");
        let projects = ProjectsStore::load(data_root.projects_file())
            .await
            .unwrap();
        let sessions = SessionsStore::load(data_root.clone()).await.unwrap();
        let app = build_router(
            AppState::new(data_root, projects, sessions, Arc::new(EventBus::default()))
                .with_tool_keys_file(&keys_file),
        );

        let listed = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/tool-keys")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(listed.status(), StatusCode::OK);
        let listed = json(listed).await;
        assert_eq!(listed.as_array().unwrap().len(), 3, "{listed}");
        assert!(listed
            .as_array()
            .unwrap()
            .iter()
            .all(|key| key["set"] == false));

        let written = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/tool-keys/brave")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"value":"secret"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(written.status(), StatusCode::OK);
        assert_eq!(json(written).await["set"], true);
        let text = std::fs::read_to_string(&keys_file).unwrap();
        assert!(text.contains("\"BRAVE_API_KEY\": \"secret\""), "{text}");

        let listed = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/tool-keys")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let listed = json(listed).await;
        assert_eq!(listed[0]["name"], "brave");
        assert_eq!(listed[0]["set"], true);

        let cleared = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/tool-keys/brave")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"value":""}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(json(cleared).await["set"], false);
        assert!(!std::fs::read_to_string(&keys_file)
            .unwrap()
            .contains("BRAVE_API_KEY"));

        let unknown = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/tool-keys/nope")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"value":"x"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn prompt_overrides_are_listed_written_and_deleted() {
        let app = app().await;

        let listed = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/prompts")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(listed.status(), StatusCode::OK);
        let listed = json(listed).await;
        let policy = listed
            .as_array()
            .unwrap()
            .iter()
            .find(|prompt| prompt["id"] == "policy")
            .unwrap();
        assert!(policy["overrideHash"].is_null());
        assert_eq!(policy["differs"], false);

        let written = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/prompts/policy")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"text":"Custom {{ project_name }}"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(written.status(), StatusCode::OK);
        let written = json(written).await;
        assert_eq!(written["differs"], true);
        assert_eq!(written["activeHash"], written["overrideHash"]);

        let fetched = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/prompts/policy")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(fetched.status(), StatusCode::OK);
        assert_eq!(json(fetched).await["text"], "Custom {{ project_name }}");
        let deleted = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/api/prompts/policy")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(deleted.status(), StatusCode::OK);
        let deleted = json(deleted).await;
        assert!(deleted["overrideHash"].is_null());
        assert_eq!(deleted["differs"], false);
        assert_eq!(deleted["activeHash"], deleted["defaultHash"]);
    }

    struct RecordingDriver {
        woken: std::sync::Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl crate::driver::SessionDriver for RecordingDriver {
        async fn prompt(
            &self,
            _session_id: &str,
            _text: &str,
            _images: Vec<PromptImageDto>,
        ) -> Result<(), String> {
            Ok(())
        }
        async fn cancel(&self, _session_id: &str) -> Result<(), String> {
            Ok(())
        }
        async fn wake(&self, _session_id: &str, text: &str) -> Result<(), String> {
            self.woken.lock().unwrap().push(text.to_string());
            Ok(())
        }
        async fn connect(&self, _session_id: &str) -> Result<(), String> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn a_steer_on_an_idle_session_is_woken_once_and_marked_delivered() {
        let dir = tempdir().unwrap();
        let data_root = DataRoot::new(dir.path());
        let projects = ProjectsStore::load(data_root.projects_file())
            .await
            .unwrap();
        projects
            .insert(store::Project {
                id: "proj_1".into(),
                name: "demo".into(),
                root: dir.path().to_path_buf(),
            })
            .await
            .unwrap();
        let sessions = SessionsStore::load(data_root.clone()).await.unwrap();
        sessions
            .insert(SessionMeta {
                id: "ses_1".into(),
                project_id: "proj_1".into(),
                title: "Steer".into(),
                ponytail_level: "full".into(),
                created_at: 1,
                updated_at: 1,
                ..SessionMeta::default()
            })
            .await
            .unwrap();
        let driver = Arc::new(RecordingDriver {
            woken: std::sync::Mutex::new(Vec::new()),
        });
        let bus = Arc::new(EventBus::default());
        let mut published = bus.subscribe();
        let app = build_router(
            AppState::new(data_root, projects, sessions, Arc::clone(&bus))
                .with_driver(driver.clone()),
        );

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/sessions/ses_1/steer")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"text":"steer: look at line 4"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            driver.woken.lock().unwrap().as_slice(),
            ["steer: look at line 4"]
        );

        let mut delivered = false;
        while let Ok(event) = published.try_recv() {
            if event.kind == emberloop_core::events::EventKind::SteerDelivered {
                assert_eq!(event.data["route"], "sent at idle");
                delivered = true;
            }
        }
        assert!(delivered, "steer_delivered event published");
    }

    #[tokio::test]
    async fn a_prompt_during_a_turn_is_queued_as_a_steer() {
        let dir = tempdir().unwrap();
        let data_root = DataRoot::new(dir.path());
        let projects = ProjectsStore::load(data_root.projects_file())
            .await
            .unwrap();
        let sessions = SessionsStore::load(data_root.clone()).await.unwrap();
        sessions
            .insert(SessionMeta {
                id: "ses_1".into(),
                project_id: "proj_1".into(),
                title: "Busy".into(),
                ..SessionMeta::default()
            })
            .await
            .unwrap();
        let driver = Arc::new(RecordingDriver {
            woken: std::sync::Mutex::new(Vec::new()),
        });
        let bus = Arc::new(EventBus::default());
        let mut published = bus.subscribe();
        let state =
            AppState::new(data_root, projects, sessions, Arc::clone(&bus)).with_driver(driver);
        state.steers.queue("ses_1").set_turn_active(true);
        let app = build_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/sessions/ses_1/prompt")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"text":"hold on","images":[]}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let mut queued = 0;
        while let Ok(event) = published.try_recv() {
            assert_ne!(event.kind, emberloop_core::events::EventKind::Prompt);
            if event.kind == emberloop_core::events::EventKind::SteerQueued {
                assert_eq!(event.data["text"], "hold on");
                queued += 1;
            }
        }
        assert_eq!(queued, 1, "the message waits in the steer queue");
    }

    #[tokio::test]
    async fn unknown_placeholder_is_a_bad_request() {
        let app = app().await;
        let response = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/prompts/policy")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"text":"Custom {{ nope }}"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
    #[tokio::test]
    async fn title_settings_default_and_validate_the_sdk_model_id() {
        let app = app().await;
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/settings")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let settings = json(response).await;
        assert_eq!(settings["title_model"], "openrouter/openrouter/free");
        assert_eq!(settings["title_generation"], true);
        let response = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/settings")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"title_model":"not-a-provider-model"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}
