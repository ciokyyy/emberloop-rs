//! Filesystem helpers behind `skill_manage` and the `view` log: per-skill locks, backups, the
//! usage log and the rule for where a supporting file may sit.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;

use crate::tag::content_hash;
use crate::{ToolError, ToolResult, Toolbox};

/// Backups kept per skill; the oldest go first.
const KEEP_BACKUPS: usize = 10;
/// The only folders a supporting file may sit in.
pub const SUPPORT_DIRS: [&str; 4] = ["references", "templates", "scripts", "assets"];
/// One JSON line per `skill_manage` call and per `view`, in each skills root.
pub const USAGE_LOG: &str = ".usage.jsonl";

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// Exclusive lock on one skill name under `<root>/.locks`, held until the returned file drops.
pub fn lock(root: &Path, name: &str) -> io::Result<File> {
    let dir = root.join(".locks");
    fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.lock", content_hash(name.as_bytes())));
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(path)?;
    file.lock()?;
    Ok(file)
}

/// Copies `bytes` to `<root>/.backup/<name>/<unix-seconds>-<file>`, then keeps the newest backups.
pub fn backup(root: &Path, name: &str, file: &str, bytes: &[u8]) -> io::Result<()> {
    let dir = root.join(".backup").join(name);
    fs::create_dir_all(&dir)?;
    let mut secs = now_secs();
    while dir.join(format!("{secs}-{file}")).exists() {
        secs += 1;
    }
    fs::write(dir.join(format!("{secs}-{file}")), bytes)?;
    prune(&dir)
}

fn prune(dir: &Path) -> io::Result<()> {
    let mut backups: Vec<(u64, PathBuf)> = fs::read_dir(dir)?
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let secs = path
                .file_name()?
                .to_str()?
                .split('-')
                .next()?
                .parse()
                .ok()?;
            Some((secs, path))
        })
        .collect();
    backups.sort();
    let extra = backups.len().saturating_sub(KEEP_BACKUPS);
    for (_, path) in backups.drain(..extra) {
        fs::remove_file(path)?;
    }
    Ok(())
}

/// Appends one line to `<root>/.usage.jsonl`: when, which skill, which op and the result.
pub fn log_usage(root: &Path, name: &str, op: &str, result: &str) -> io::Result<()> {
    fs::create_dir_all(root)?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join(USAGE_LOG))?;
    writeln!(
        file,
        "{}",
        json!({"time": now_secs(), "name": name, "op": op, "result": result})
    )
}

/// Logs a `view` when a read names a SKILL.md directly under a skill root. The path is taken as
/// requested, so a symlinked skill logs under the root it is linked from.
pub fn log_view(toolbox: &Toolbox, requested: &str) {
    let path = toolbox.root().join(requested);
    let (Some(dir), Some(root)) = (path.parent(), path.parent().and_then(Path::parent)) else {
        return;
    };
    let named = path.file_name().is_some_and(|file| file == "SKILL.md");
    let Some(name) = dir.file_name().and_then(|name| name.to_str()) else {
        return;
    };
    let Ok(canonical_root) = fs::canonicalize(root) else {
        return;
    };
    let roots = [
        toolbox.global_skills().map(Path::to_path_buf),
        Some(toolbox.root().join(".agents/skills")),
    ];
    let under_skill_root = roots
        .iter()
        .flatten()
        .any(|skills| fs::canonicalize(skills).is_ok_and(|skills| skills == canonical_root));
    if named && under_skill_root {
        if let Err(error) = log_usage(root, name, "view", "ok") {
            tracing::warn!(%error, "could not log the skill view");
        }
    }
}

/// Resolves a supporting file under `dir`: relative, under one of SUPPORT_DIRS, with no `..`, and
/// no symlink on the way. Parents may not exist yet; callers check the escape after creating them.
pub fn support_path(dir: &Path, rel: &str) -> ToolResult<PathBuf> {
    let mut parts = Vec::new();
    for component in Path::new(rel).components() {
        match component {
            Component::Normal(part) => parts.push(part),
            _ => {
                return Err(ToolError::msg(format!(
                    "path {rel:?} must be relative, with no .. and no leading /"
                )))
            }
        }
    }
    let folder = parts.first().and_then(|part| part.to_str()).unwrap_or("");
    if parts.len() < 2 || !SUPPORT_DIRS.contains(&folder) {
        return Err(ToolError::msg(format!(
            "path {rel:?} must be a file under references/, templates/, scripts/ or assets/"
        )));
    }
    let mut walked = dir.to_path_buf();
    for part in &parts {
        walked.push(part);
        if walked
            .symlink_metadata()
            .is_ok_and(|meta| meta.file_type().is_symlink())
        {
            return Err(ToolError::msg(format!(
                "{} is a symlink; supporting files must stay inside the skill",
                walked.display()
            )));
        }
    }
    Ok(dir.join(rel))
}
