//! The `graph` MCP server's naming rule: every codebase-memory query tool takes
//! the indexed project, and a Session's root decides which project that is.

use std::path::{Path, PathBuf};

use serde_json::Value;

/// The server name whose calls get a `project` argument.
pub const SERVER: &str = "graph";

/// The codebase-memory-mcp executable, native build.
const BINARY: &str = "codebase-memory-mcp";

/// Whether an entry is the daemon's `npx -y codebase-memory-mcp@<version>` fallback.
pub fn is_npx_fallback(command: Option<&str>, args: &[String]) -> bool {
    let is_npx = command.is_some_and(|command| {
        Path::new(command)
            .file_name()
            .is_some_and(|name| name == "npx")
    });
    is_npx
        && matches!(args, [flag, package] if flag == "-y" && package.starts_with("codebase-memory-mcp@"))
}

/// Where a native binary is looked for: every `PATH` entry, then `~/.local/bin`.
pub fn default_bin_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join(".local").join("bin"));
    }
    dirs
}

/// The native binary in the first of `dirs` that holds one.
pub fn native_binary(dirs: &[PathBuf]) -> Option<PathBuf> {
    dirs.iter()
        .map(|dir| dir.join(BINARY))
        .find(|candidate| candidate.is_file())
}

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
