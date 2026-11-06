//! Providers endpoints and the login seam. The daemon owns the agy process and
//! the Account home; the API only reads the view and passes the User's actions.

use async_trait::async_trait;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::state::AppState;

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct AccountView {
    pub id: String,
    pub label: String,
    pub tier: String,
    pub signed_in: bool,
    pub last_login: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProviderView {
    pub id: String,
    pub name: String,
    pub signed_in: bool,
    pub accounts: Vec<AccountView>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct LoginView {
    pub id: String,
    pub state: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub url: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub output: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub error: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub tier: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LoginRequest {
    #[serde(default)]
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CompleteLoginRequest {
    pub address: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SetKeyRequest {
    pub key: String,
}

#[derive(Debug, Clone)]
pub struct LoginError {
    pub status: StatusCode,
    pub message: String,
}

impl LoginError {
    pub fn conflict(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            message: message.into(),
        }
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: message.into(),
        }
    }

    pub fn unavailable(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            message: message.into(),
        }
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }

    pub fn server(error: impl std::fmt::Display) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: error.to_string(),
        }
    }
}

impl IntoResponse for LoginError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(serde_json::json!({ "error": self.message })),
        )
            .into_response()
    }
}

#[async_trait]
pub trait ProviderLogin: Send + Sync {
    async fn providers(&self) -> Vec<ProviderView>;
    async fn start(&self, label: &str) -> Result<LoginView, LoginError>;
    async fn status(&self, id: &str) -> Option<LoginView>;
    async fn cancel(&self, id: &str) -> Result<(), LoginError>;
    async fn complete(&self, id: &str, address: &str) -> Result<LoginView, LoginError>;
    async fn sign_out(&self, account_id: &str) -> Result<(), LoginError>;
    async fn set_key(&self, _provider: &str, _key: &str) -> Result<(), LoginError> {
        Err(LoginError::unavailable(
            "the daemon does not store provider keys",
        ))
    }
}

fn login(state: &AppState) -> Result<&std::sync::Arc<dyn ProviderLogin>, LoginError> {
    state
        .provider_login
        .as_ref()
        .ok_or_else(|| LoginError::unavailable("the daemon has no provider login"))
}

#[utoipa::path(
    get,
    path = "/api/providers",
    responses((status = 200, body = [ProviderView]), (status = 503, description = "no login seam"))
)]
pub async fn list_providers(
    State(state): State<AppState>,
) -> Result<Json<Vec<ProviderView>>, LoginError> {
    Ok(Json(login(&state)?.providers().await))
}

#[utoipa::path(
    post,
    path = "/api/providers/agy/login",
    request_body = LoginRequest,
    responses(
        (status = 201, body = LoginView),
        (status = 409, description = "a login already runs"),
        (status = 503, description = "the agy server is not installed")
    )
)]
pub async fn start_agy_login(
    State(state): State<AppState>,
    Json(request): Json<LoginRequest>,
) -> Result<(StatusCode, Json<LoginView>), LoginError> {
    let view = login(&state)?.start(&request.label).await?;
    Ok((StatusCode::CREATED, Json(view)))
}

#[utoipa::path(
    get,
    path = "/api/providers/agy/login/{id}",
    params(("id" = String, Path, description = "login id")),
    responses((status = 200, body = LoginView), (status = 404, description = "unknown login"))
)]
pub async fn get_agy_login(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<LoginView>, LoginError> {
    login(&state)?
        .status(&id)
        .await
        .map(Json)
        .ok_or_else(|| LoginError::not_found("the Antigravity login is not running"))
}

#[utoipa::path(
    delete,
    path = "/api/providers/agy/login/{id}",
    params(("id" = String, Path, description = "login id")),
    responses((status = 200, description = "canceled"), (status = 404, description = "unknown login"))
)]
pub async fn cancel_agy_login(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, LoginError> {
    login(&state)?.cancel(&id).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[utoipa::path(
    post,
    path = "/api/providers/agy/login/{id}/complete",
    params(("id" = String, Path, description = "login id")),
    request_body = CompleteLoginRequest,
    responses(
        (status = 200, body = LoginView),
        (status = 400, description = "the pasted address cannot complete the login"),
        (status = 404, description = "unknown login"),
        (status = 409, description = "the login is not running")
    )
)]
pub async fn complete_agy_login(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<CompleteLoginRequest>,
) -> Result<Json<LoginView>, LoginError> {
    login(&state)?
        .complete(&id, &request.address)
        .await
        .map(Json)
}

#[utoipa::path(
    delete,
    path = "/api/providers/agy/accounts/{id}",
    params(("id" = String, Path, description = "account id")),
    responses((status = 200, description = "signed out"), (status = 404, description = "unknown account"))
)]
pub async fn sign_out_agy_account(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, LoginError> {
    login(&state)?.sign_out(&id).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[utoipa::path(
    put,
    path = "/api/providers/opencode-go/key",
    request_body = SetKeyRequest,
    responses((status = 200, description = "saved"), (status = 400, description = "empty key"))
)]
pub async fn set_opencode_key(
    State(state): State<AppState>,
    Json(request): Json<SetKeyRequest>,
) -> Result<Json<serde_json::Value>, LoginError> {
    login(&state)?.set_key("opencode-go", &request.key).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/providers", get(list_providers))
        .route("/api/providers/agy/login", post(start_agy_login))
        .route(
            "/api/providers/agy/login/{id}",
            get(get_agy_login).delete(cancel_agy_login),
        )
        .route(
            "/api/providers/agy/login/{id}/complete",
            post(complete_agy_login),
        )
        .route(
            "/api/providers/agy/accounts/{id}",
            delete(sign_out_agy_account),
        )
        .route("/api/providers/opencode-go/key", put(set_opencode_key))
}
