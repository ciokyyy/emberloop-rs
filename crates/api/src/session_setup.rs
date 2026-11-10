//! Validate model settings and image input before touching a harness or persisting metadata.
use crate::{routes::PromptImageDto, state::AppState};
use axum::http::StatusCode;
use store::SessionMeta;

type ApiError = (StatusCode, String);

pub async fn configure(
    state: &AppState,
    meta: &mut SessionMeta,
    model: Option<String>,
    effort: Option<String>,
) -> Result<(), ApiError> {
    if state.steers.queue(&meta.id).is_turn_active() {
        return Err((
            StatusCode::CONFLICT,
            "Wait for the current turn before changing the model or reasoning effort".into(),
        ));
    }
    let changed_model = model.as_ref().is_some_and(|id| id != &meta.model);
    let selected = model.as_deref().unwrap_or(&meta.model);
    let entries = state.models.entries().await;
    let old_source = entries
        .iter()
        .find(|entry| entry.id == meta.model)
        .map(|entry| entry.source.clone());
    let entry = entries
        .into_iter()
        .find(|entry| entry.id == selected)
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                format!("Unknown model: {selected}"),
            )
        })?;
    let level = match effort {
        Some(level) => (!level.is_empty()).then_some(level),
        None if !changed_model => meta.thought_level.clone(),
        None => None,
    };
    if let Some(level) = &level {
        if !entry.efforts.contains(level) {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("{selected} does not support reasoning effort {level}"),
            ));
        }
    }
    // New Sessions have no harness to configure; their first prompt uses the saved model.
    if state.sessions.get(&meta.id).await.is_some() {
        state
            .driver
            .configure_model(&meta.id, selected, level.as_deref())
            .await
            .map_err(|error| (StatusCode::CONFLICT, error))?;
    }
    if old_source.as_deref() != Some(entry.source.as_str()) {
        // Another harness cannot load the old conversation; it starts fresh and the proxy carries the history.
        meta.resume_token = None;
    }
    meta.model = selected.to_string();
    meta.thought_level = level;
    Ok(())
}

pub async fn validate_images(
    state: &AppState,
    meta: &SessionMeta,
    images: &[PromptImageDto],
) -> Result<(), ApiError> {
    if images.is_empty() {
        return Ok(());
    }
    if images.len() > 5 || images.iter().any(invalid_image) {
        return Err((
            StatusCode::BAD_REQUEST,
            "Attach up to 5 PNG, JPEG, WebP or GIF images, no larger than 10 MiB each".into(),
        ));
    }
    if state
        .models
        .entries()
        .await
        .iter()
        .any(|model| model.id == meta.model && !model.image_input)
    {
        return Err((
            StatusCode::BAD_REQUEST,
            "This model does not support images".into(),
        ));
    }
    Ok(())
}

fn invalid_image(image: &PromptImageDto) -> bool {
    !matches!(
        image.mime_type.as_str(),
        "image/png" | "image/jpeg" | "image/webp" | "image/gif"
    ) || image.data_base64.len() > (10 * 1024 * 1024usize).div_ceil(3) * 4
}
