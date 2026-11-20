//! Read-only skill pages: every skill the daemon sees, one skill in full, and its change history.

use std::path::PathBuf;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use tools::skills::{self, Skill};
use tools::skillview::{self, Detail, History, Summary};

use crate::state::AppState;

type Failure = (StatusCode, String);

/// Search hits per request when `q` is set, and the hard cap.
const DEFAULT_SEARCH_LIMIT: usize = 20;
const MAX_SEARCH_LIMIT: usize = 50;

/// `?project=<id>` narrows the list to one project's root plus the global root; `?q=` ranks the
/// skills and `?limit=` caps the hits.
#[derive(Deserialize)]
pub struct SkillsQuery {
    project: Option<String>,
    q: Option<String>,
    limit: Option<usize>,
}

/// The global skills root and every project root, the same roots a skill write reaches.
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
    let scoped = query.project.is_some();
    if let Some(id) = query.project {
        projects = state
            .projects
            .get(&id)
            .await
            .map(|project| vec![project.root])
            .unwrap_or_default();
    }
    let found = skillview::list(global.as_deref(), &projects);
    let text = query.q.unwrap_or_default();
    let text = text.trim();
    if text.is_empty() {
        return Json(found.iter().map(skillview::summary).collect());
    }
    let modules = match (scoped, projects.first()) {
        (true, Some(project)) => skills::modules(global.as_deref(), project),
        _ => Vec::new(),
    };
    Json(ranked(&found, &modules, text, query.limit))
}

/// Prefix matches alphabetically first, then the rest by BM25 score above 0, capped by `limit`.
fn ranked(
    found: &[Skill],
    modules: &[skills::Module],
    text: &str,
    limit: Option<usize>,
) -> Vec<Summary> {
    // `skills::valid_name` keeps names lowercase ASCII, so only the query needs folding.
    let needle = text.to_lowercase();
    let mut ranked: Vec<&Skill> = found
        .iter()
        .filter(|skill| skill.name.starts_with(&needle))
        .collect();
    ranked.sort_by(|left, right| left.name.cmp(&right.name));
    ranked.extend(
        tools::skillsearch::score(found, modules, text)
            .into_iter()
            .map(|(index, _)| &found[index])
            .filter(|skill| !skill.name.starts_with(&needle)),
    );
    let limit = limit
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_SEARCH_LIMIT)
        .min(MAX_SEARCH_LIMIT);
    ranked
        .into_iter()
        .take(limit)
        .map(skillview::summary)
        .collect()
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
