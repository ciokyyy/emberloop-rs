//! Read-only skill pages: every skill the daemon sees, one skill in full, and its change history.

use std::path::PathBuf;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use tools::skills::Skill;
use tools::skillview::{self, Detail, History, Summary};

use crate::state::AppState;

type Failure = (StatusCode, String);

/// `?project=<id>` narrows the list to one project's root plus the global root.
#[derive(Deserialize)]
pub struct SkillsQuery {
    project: Option<String>,
}

/// The global skills root and every project root, the same roots `skill_manage` writes to.
async fn roots(state: &AppState) -> (Option<PathBuf>, Vec<PathBuf>) {
    let projects = state
        .projects
        .list()
        .await
        .into_iter()
        .map(|p| p.root)
        .collect();
    (state.global_skills.clone(), projects)
}

/// Resolves a name against every root, so a project skill shadows a global one of the same name.
async fn find(state: &AppState, name: &str) -> Result<Skill, Failure> {
    let (global, projects) = roots(state).await;
    // shortcut: a name held by a project and the global root resolves to the first one listed,
    // the project skill; a path-based lookup would need the UI to carry a path, upgrade if needed.
    skillview::list(global.as_deref(), &projects)
        .into_iter()
        .find(|skill| skill.name == name)
        .ok_or((
            StatusCode::NOT_FOUND,
            format!("no agent skill named {name}"),
        ))
}

pub async fn list_skills(
    State(state): State<AppState>,
    Query(query): Query<SkillsQuery>,
) -> Json<Vec<Summary>> {
    let (global, mut projects) = roots(&state).await;
    if let Some(id) = query.project {
        projects = state
            .projects
            .get(&id)
            .await
            .map(|project| vec![project.root])
            .unwrap_or_default();
    }
    let found = skillview::list(global.as_deref(), &projects);
    Json(found.iter().map(skillview::summary).collect())
}

pub async fn skill_detail(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<Detail>, Failure> {
    let skill = find(&state, &name).await?;
    skillview::detail(&skill).map(Json).ok_or((
        StatusCode::NOT_FOUND,
        format!("skill {name} has no readable SKILL.md"),
    ))
}

pub async fn skill_history(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<History>, Failure> {
    let skill = find(&state, &name).await?;
    Ok(Json(skillview::history(&skill)))
}
