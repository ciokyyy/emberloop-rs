//! Read-only file serving for the Session feed: an answer names a file, the UI shows it.
//! Only files inside the Session's Project root or its own Session directory are served.

use std::path::{Path as FsPath, PathBuf};

use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use crate::state::AppState;

/// Larger files are refused: the browser would hold the whole body in memory.
const MAX_BYTES: u64 = 10 * 1024 * 1024;

#[derive(Deserialize)]
pub struct FileQuery {
    /// An absolute path, or one relative to the Project root.
    path: String,
}

/// Content type by extension. Source and data files go out as `text/plain` so a browser never
/// runs them; shortcut: a fixed table, upgrade to a mime crate when the list keeps growing.
fn content_type(path: &FsPath) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "html" | "htm" => "text/html; charset=utf-8",
        "pdf" => "application/pdf",
        "json" | "jsonl" | "csv" | "tsv" | "txt" | "log" | "md" | "markdown" | "yaml" | "yml"
        | "toml" | "xml" | "ini" | "env" | "js" | "mjs" | "ts" | "tsx" | "jsx" | "css" | "rs"
        | "py" | "go" | "sh" | "sql" | "svelte" | "diff" | "patch" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// `path` resolved to a real file inside one of `roots`, or `None`. `canonicalize` follows
/// symlinks and `..`, so a link that points out of a root is refused by the prefix check.
fn resolve(path: &str, roots: &[PathBuf]) -> Option<PathBuf> {
    let base = roots.first()?;
    let given = PathBuf::from(path.strip_prefix("file://").unwrap_or(path));
    let full = if given.is_absolute() {
        given
    } else {
        base.join(given)
    };
    let real = full.canonicalize().ok()?;
    roots
        .iter()
        .filter_map(|root| root.canonicalize().ok())
        .any(|root| real.starts_with(root))
        .then_some(real)
}

pub async fn session_file(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<FileQuery>,
) -> Result<Response, (StatusCode, String)> {
    let session = state
        .sessions
        .get(&id)
        .await
        .ok_or((StatusCode::NOT_FOUND, format!("Session {id} not found")))?;
    let mut roots = Vec::new();
    if let Some(project) = state.projects.get(&session.project_id).await {
        roots.push(project.root);
    }
    roots.push(state.data_root.session_dir(&id));
    let real = resolve(&query.path, &roots)
        .ok_or((StatusCode::NOT_FOUND, "file not found".to_string()))?;
    let meta = tokio::fs::metadata(&real)
        .await
        .map_err(|_| (StatusCode::NOT_FOUND, "file not found".to_string()))?;
    if !meta.is_file() {
        return Err((StatusCode::NOT_FOUND, "not a file".to_string()));
    }
    if meta.len() > MAX_BYTES {
        return Err((StatusCode::PAYLOAD_TOO_LARGE, "file too large".to_string()));
    }
    let body = tokio::fs::read(&real)
        .await
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    Ok((
        [
            (header::CONTENT_TYPE, content_type(&real)),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            // HTML and SVG from a Project are untrusted: no scripts, no daemon-origin access.
            (header::CONTENT_SECURITY_POLICY, "sandbox"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        body,
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_files_under_a_root_resolve() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("a.txt"), "x").unwrap();
        std::fs::write(dir.path().join("secret.txt"), "s").unwrap();
        let roots = vec![root.clone()];
        assert!(resolve("a.txt", &roots).is_some());
        assert!(resolve(&format!("file://{}/a.txt", root.display()), &roots).is_some());
        assert!(resolve("../secret.txt", &roots).is_none());
        assert!(resolve(&dir.path().join("secret.txt").to_string_lossy(), &roots).is_none());
        assert!(resolve("missing.txt", &roots).is_none());
    }

    #[test]
    fn unknown_extensions_are_not_rendered() {
        assert_eq!(
            content_type(FsPath::new("a.json")),
            "text/plain; charset=utf-8"
        );
        assert_eq!(content_type(FsPath::new("a.PNG")), "image/png");
        assert_eq!(
            content_type(FsPath::new("a.exe")),
            "application/octet-stream"
        );
    }
}
