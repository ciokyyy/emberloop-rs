//! The `graph` MCP server's naming rule: every codebase-memory query tool takes
//! the indexed project, and a Session's root decides which project that is.

use std::path::Path;

use serde_json::Value;

/// The server name whose calls get a `project` argument.
pub const SERVER: &str = "graph";

/// codebase-memory names a project after its absolute path with the separators
/// turned into `-` and the leading one dropped: `/tmp/x` is `tmp-x`.
pub fn project_from_path(root: &Path) -> String {
    root.to_string_lossy()
        .trim_start_matches(['/', '\\'])
        .replace(['/', '\\'], "-")
}

/// The name `list_projects` reports for `root`, when it knows the root.
pub fn match_project(output: &str, root: &Path) -> Option<String> {
    let value: Value = serde_json::from_str(output).ok()?;
    let projects = value.get("projects")?.as_array()?;
    projects.iter().find_map(|project| {
        let path = Path::new(project.get("root_path")?.as_str()?);
        let name = project.get("name")?.as_str()?;
        same_root(path, root).then(|| name.to_string())
    })
}

fn same_root(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}
