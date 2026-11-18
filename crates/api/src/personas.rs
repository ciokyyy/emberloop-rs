//! Goldfish personas: the read-only list page and the one resolver the proxy calls.

use std::path::Path;

use axum::extract::{Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use tools::personas::{self, Persona};

use crate::state::AppState;

/// The system prompt (frontmatter stripped) of persona `persona`, read from disk now, or `None`
/// when no search level holds it. The proxy calls this to build a goldfish request.
pub fn resolve_persona(
    global_skills: Option<&Path>,
    project_root: &Path,
    persona: &str,
) -> Option<String> {
    personas::resolve(global_skills, project_root, persona)
}

/// Personas visible from `project_root`, in search order.
pub fn available(global_skills: Option<&Path>, project_root: &Path) -> Vec<Persona> {
    personas::discover(global_skills, project_root)
}

/// `?project=<id>` picks the project whose roots are searched.
#[derive(Deserialize)]
pub struct PersonasQuery {
    project: Option<String>,
}

#[derive(Serialize)]
pub struct PersonaSummary {
    name: String,
    description: String,
    path: String,
}

pub async fn list_personas(
    State(state): State<AppState>,
    Query(query): Query<PersonasQuery>,
) -> Json<Vec<PersonaSummary>> {
    let root = match query.project {
        Some(id) => state.projects.get(&id).await.map(|project| project.root),
        None => None,
    };
    let found = match root {
        Some(root) => available(state.global_skills.as_deref(), &root),
        None => Vec::new(),
    };
    Json(
        found
            .into_iter()
            .map(|persona| PersonaSummary {
                name: persona.name,
                description: persona.description,
                path: persona.path.to_string_lossy().into_owned(),
            })
            .collect(),
    )
}
