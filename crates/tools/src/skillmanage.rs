//! `skill_manage`: the agent writes its own skills. One call applies an ordered list of ops
//! all or nothing; only agent-origin skills change; every change keeps a backup; a patch or a
//! write needs a prior read of the file it changes.

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

use crate::skills::{self, Front, Skill};
use crate::skillstore;
use crate::tag::content_hash;
use crate::{ToolError, ToolResult, Toolbox};

const MAX_SKILL_CHARS: usize = 100_000;
const MAX_SUPPORT_BYTES: usize = 1 << 20;
const MAX_DESCRIPTION_CHARS: usize = 1024;

#[derive(Deserialize)]
struct Args {
    ops: Vec<Op>,
}

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum Op {
    Create(CreateOp),
    Patch(PatchOp),
    WriteFile(FileOp),
    RemoveFile(FileOp),
}

#[derive(Deserialize)]
struct CreateOp {
    name: String,
    content: String,
    #[serde(default)]
    scope: Option<String>,
}

#[derive(Deserialize)]
struct PatchOp {
    name: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    old_string: Option<String>,
    #[serde(default)]
    new_string: Option<String>,
    #[serde(default)]
    replace_all: bool,
    /// Whole-file replacement: the last resort after a targeted patch.
    #[serde(default)]
    content: Option<String>,
}

#[derive(Deserialize)]
struct FileOp {
    name: String,
    path: String,
    #[serde(default)]
    content: Option<String>,
}

impl Op {
    fn name(&self) -> &str {
        match self {
            Op::Create(op) => &op.name,
            Op::Patch(op) => &op.name,
            Op::WriteFile(op) | Op::RemoveFile(op) => &op.name,
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            Op::Create(_) => "create",
            Op::Patch(_) => "patch",
            Op::WriteFile(_) => "write_file",
            Op::RemoveFile(_) => "remove_file",
        }
    }
}

/// One step a failed call reverses. Steps run newest first.
enum Undo {
    /// A file that existed before the call: its old bytes go back.
    Restore { path: PathBuf, bytes: Vec<u8> },
    /// A file the call created.
    Remove(PathBuf),
    /// A folder the call created.
    RemoveDir(PathBuf),
}

/// `skill_manage`: applies the ops in order, all or nothing. Logs one usage line per call.
pub fn run(toolbox: &Toolbox, args: &Value) -> ToolResult<String> {
    let args: Args = serde_json::from_value(args.clone())?;
    let Some(first) = args.ops.first() else {
        return Err(ToolError::msg(
            "ops is required: give one or more operations",
        ));
    };
    let result = apply_all(toolbox, &args.ops);
    if let Ok(root) = root_for(toolbox, first).or_else(|_| scope_root(toolbox, None)) {
        let outcome = outcome(&result);
        if let Err(error) = skillstore::log_usage(&root, first.name(), first.kind(), &outcome) {
            tracing::warn!(%error, "could not log the skill_manage call");
        }
    }
    result
}

fn outcome(result: &ToolResult<String>) -> String {
    match result {
        Ok(_) => "ok".to_string(),
        Err(error) => error.to_string(),
    }
}

/// Takes every lock the ops need, applies them, and rolls back on the first failure.
fn apply_all(toolbox: &Toolbox, ops: &[Op]) -> ToolResult<String> {
    let keys = lock_keys(toolbox, ops)?;
    let _locks = keys
        .iter()
        .map(|(root, name)| skillstore::lock(root, name))
        .collect::<io::Result<Vec<File>>>()?;
    let mut undo = Vec::new();
    match ops.iter().try_for_each(|op| apply(toolbox, op, &mut undo)) {
        Ok(()) => Ok(format!("applied {} op(s)\n", ops.len())),
        Err(error) => {
            let failures = rollback(undo);
            if failures.is_empty() {
                return Err(error);
            }
            Err(ToolError::msg(format!(
                "{error}; rollback incomplete: {}",
                failures.join("; ")
            )))
        }
    }
}

/// Reverses `steps`, newest first. Returns one line per step that could not be reversed.
fn rollback(steps: Vec<Undo>) -> Vec<String> {
    steps
        .into_iter()
        .rev()
        .filter_map(|step| {
            let (path, result) = match &step {
                Undo::Restore { path, bytes } => (path, fs::write(path, bytes)),
                Undo::Remove(path) => (path, fs::remove_file(path)),
                Undo::RemoveDir(path) => (path, fs::remove_dir(path)),
            };
            match result {
                Err(error) if error.kind() != io::ErrorKind::NotFound => {
                    Some(format!("could not undo {}: {error}", path.display()))
                }
                _ => None,
            }
        })
        .collect()
}

/// Each (root, name) the ops touch, once and sorted, so every call takes its locks in one order.
fn lock_keys(toolbox: &Toolbox, ops: &[Op]) -> ToolResult<Vec<(PathBuf, String)>> {
    let mut keys = BTreeSet::new();
    for op in ops {
        check_name(op.name())?;
        keys.insert((root_for(toolbox, op)?, op.name().to_string()));
    }
    Ok(keys.into_iter().collect())
}

fn check_name(name: &str) -> ToolResult<()> {
    if skills::valid_name(name) {
        return Ok(());
    }
    Err(ToolError::msg(format!(
        "skill name {name:?} must start with a-z or 0-9 and use only a-z, 0-9, '.', '_' or '-' (at most 64)"
    )))
}

/// The skills root an op writes in: `create` takes its scope, other ops follow the skill there.
fn root_for(toolbox: &Toolbox, op: &Op) -> ToolResult<PathBuf> {
    match op {
        Op::Create(op) => scope_root(toolbox, op.scope.as_deref()),
        other => Ok(root_of(&locate(toolbox, other.name())?)),
    }
}

fn scope_root(toolbox: &Toolbox, scope: Option<&str>) -> ToolResult<PathBuf> {
    match scope.unwrap_or("global") {
        "global" => toolbox
            .global_skills()
            .map(Path::to_path_buf)
            .ok_or_else(|| ToolError::msg("no global skills root is configured")),
        "project" => Ok(toolbox.root().join(".agents/skills")),
        other => Err(ToolError::msg(format!(
            "scope {other:?} must be \"global\" or \"project\""
        ))),
    }
}

/// The skill `name` as discovery sees it: a project skill wins over a global one.
fn locate(toolbox: &Toolbox, name: &str) -> ToolResult<Skill> {
    skills::locate(toolbox.global_skills(), toolbox.root(), name)
        .ok_or_else(|| ToolError::msg(format!("no skill named {name:?}")))
}

fn root_of(skill: &Skill) -> PathBuf {
    skill
        .path
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_default()
}

fn dir_of(skill: &Skill) -> PathBuf {
    skill
        .path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default()
}

fn apply(toolbox: &Toolbox, op: &Op, undo: &mut Vec<Undo>) -> ToolResult<()> {
    match op {
        Op::Create(op) => create(toolbox, op, undo),
        Op::Patch(op) => patch(toolbox, op, undo),
        Op::WriteFile(op) => write_file(toolbox, op, undo),
        Op::RemoveFile(op) => remove_file(toolbox, op, undo),
    }
}

fn create(toolbox: &Toolbox, op: &CreateOp, undo: &mut Vec<Undo>) -> ToolResult<()> {
    let root = scope_root(toolbox, op.scope.as_deref())?;
    let taken = root.join(&op.name).exists()
        || skills::discover(toolbox.global_skills(), toolbox.root())
            .iter()
            .any(|skill| skill.name == op.name);
    if taken {
        return Err(ToolError::msg(format!(
            "skill {:?} already exists; patch it instead",
            op.name
        )));
    }
    let content = skills::with_agent_origin(&op.content)
        .ok_or_else(|| ToolError::msg("SKILL.md needs frontmatter"))?;
    check_skill_text(&op.name, &content)?;
    let dir = root.join(&op.name);
    make_dirs(&dir, undo)?;
    let file = dir.join("SKILL.md");
    undo.push(Undo::Remove(file.clone()));
    fs::write(&file, &content)?;
    record_written(toolbox, &file, content.as_bytes())
}

fn patch(toolbox: &Toolbox, op: &PatchOp, undo: &mut Vec<Undo>) -> ToolResult<()> {
    let skill = locate(toolbox, &op.name)?;
    let target = target_of(&skill, op.path.as_deref())?;
    let before = editable(toolbox, &skill, &target)?;
    let next = match &op.content {
        Some(content) => content.clone(),
        None => replaced(&before, op)?,
    };
    check_result(&skill, &target, &next)?;
    commit(toolbox, &skill, &target, &before, next.as_bytes(), undo)
}

/// The text after a targeted patch: `old_string` must match once, or every match with `replace_all`.
fn replaced(before: &[u8], op: &PatchOp) -> ToolResult<String> {
    let text = std::str::from_utf8(before)
        .map_err(|_| ToolError::msg("the file is not UTF-8 text; patch needs text"))?;
    let old = op
        .old_string
        .as_deref()
        .filter(|old| !old.is_empty())
        .ok_or_else(|| ToolError::msg("patch needs a non-empty old_string, or content"))?;
    let new = op
        .new_string
        .as_deref()
        .ok_or_else(|| ToolError::msg("patch needs new_string with old_string"))?;
    let count = text.matches(old).count();
    if count == 0 {
        return Err(ToolError::msg("old_string was not found in the file"));
    }
    if count > 1 && !op.replace_all {
        return Err(ToolError::msg(format!(
            "old_string matches {count} times; add context to make it unique or set replace_all"
        )));
    }
    Ok(text.replace(old, new))
}

fn write_file(toolbox: &Toolbox, op: &FileOp, undo: &mut Vec<Undo>) -> ToolResult<()> {
    let skill = locate(toolbox, &op.name)?;
    let target = skillstore::support_path(&dir_of(&skill), &op.path)?;
    let content = op
        .content
        .as_deref()
        .ok_or_else(|| ToolError::msg("content is required for write_file"))?;
    check_support_size(content.len())?;
    if target.exists() {
        let before = editable(toolbox, &skill, &target)?;
        return commit(toolbox, &skill, &target, &before, content.as_bytes(), undo);
    }
    require_agent(&skill)?;
    if let Some(parent) = target.parent() {
        make_dirs(parent, undo)?;
    }
    ensure_inside(&dir_of(&skill), &target)?;
    undo.push(Undo::Remove(target.clone()));
    fs::write(&target, content)?;
    record_written(toolbox, &target, content.as_bytes())
}

fn remove_file(toolbox: &Toolbox, op: &FileOp, undo: &mut Vec<Undo>) -> ToolResult<()> {
    let skill = locate(toolbox, &op.name)?;
    require_agent(&skill)?;
    let target = skillstore::support_path(&dir_of(&skill), &op.path)?;
    let before = fs::read(&target)?;
    let flat = flat_name(&dir_of(&skill), &target);
    skillstore::backup(&root_of(&skill), &skill.name, &flat, &before)?;
    undo.push(Undo::Restore {
        path: target.clone(),
        bytes: before,
    });
    fs::remove_file(&target)?;
    Ok(())
}

/// Backs up `before`, then writes `next` to `target`. Rollback puts `before` back.
fn commit(
    toolbox: &Toolbox,
    skill: &Skill,
    target: &Path,
    before: &[u8],
    next: &[u8],
    undo: &mut Vec<Undo>,
) -> ToolResult<()> {
    let flat = flat_name(&dir_of(skill), target);
    skillstore::backup(&root_of(skill), &skill.name, &flat, before)?;
    undo.push(Undo::Restore {
        path: target.to_path_buf(),
        bytes: before.to_vec(),
    });
    fs::write(target, next)?;
    record_written(toolbox, target, next)
}

/// The backup file name for `target`: its path under the skill folder, with `/` written as `__`.
fn flat_name(dir: &Path, target: &Path) -> String {
    target
        .strip_prefix(dir)
        .unwrap_or(target)
        .to_string_lossy()
        .replace('/', "__")
}

/// `SKILL.md` when no path is given; otherwise a supporting file inside the skill.
fn target_of(skill: &Skill, path: Option<&str>) -> ToolResult<PathBuf> {
    match path {
        None => Ok(skill.path.clone()),
        Some(rel) => skillstore::support_path(&dir_of(skill), rel),
    }
}

/// The bytes an op may change. The skill must be agent-origin, the file must exist, and this
/// Session must have read exactly these bytes.
fn editable(toolbox: &Toolbox, skill: &Skill, target: &Path) -> ToolResult<Vec<u8>> {
    require_agent(skill)?;
    let bytes = fs::read(target)?;
    let canonical = fs::canonicalize(target)?;
    if toolbox
        .seen()
        .unseen(&canonical, &content_hash(&bytes), &[])
        .is_none()
    {
        return Err(ToolError::msg(format!(
            "Read {} with read before changing it.",
            canonical.display()
        )));
    }
    Ok(bytes)
}

fn require_agent(skill: &Skill) -> ToolResult<()> {
    if skill.origin == "agent" {
        return Ok(());
    }
    Err(ToolError::msg(format!(
        "skill {} has origin {}; only agent-origin skills change",
        skill.name, skill.origin
    )))
}

/// Creates `dir` and its missing parents, logging each new folder for rollback.
fn make_dirs(dir: &Path, undo: &mut Vec<Undo>) -> ToolResult<()> {
    let missing: Vec<&Path> = dir.ancestors().take_while(|path| !path.exists()).collect();
    for path in missing.into_iter().rev() {
        fs::create_dir(path)?;
        undo.push(Undo::RemoveDir(path.to_path_buf()));
    }
    Ok(())
}

/// Refuses a file whose folder resolves outside the skill once the folders exist.
fn ensure_inside(dir: &Path, target: &Path) -> ToolResult<()> {
    let folder = fs::canonicalize(target.parent().unwrap_or(target))?;
    if folder.starts_with(fs::canonicalize(dir)?) {
        return Ok(());
    }
    Err(ToolError::msg(format!(
        "{} resolves outside the skill",
        target.display()
    )))
}

/// Marks the bytes just written as seen, so the next change to them needs no fresh read.
fn record_written(toolbox: &Toolbox, path: &Path, bytes: &[u8]) -> ToolResult<()> {
    let canonical = fs::canonicalize(path)?;
    toolbox
        .seen()
        .record(&canonical, &content_hash(bytes), std::iter::empty::<u32>());
    Ok(())
}

/// What a patch or write would leave: SKILL.md keeps its name and agent origin, and a supporting
/// file stays under the size limit.
fn check_result(skill: &Skill, target: &Path, next: &str) -> ToolResult<()> {
    if target != skill.path {
        return check_support_size(next.len());
    }
    let front = check_skill_text(&skill.name, next)?;
    if front.origin != "agent" {
        return Err(ToolError::msg(
            "the change must keep metadata.origin: agent",
        ));
    }
    Ok(())
}

/// SKILL.md must be under the size limit, name the skill, and carry a description of at most 1024
/// characters.
fn check_skill_text(name: &str, text: &str) -> ToolResult<Front> {
    if text.chars().count() > MAX_SKILL_CHARS {
        return Err(ToolError::msg(format!(
            "SKILL.md is over {MAX_SKILL_CHARS} characters"
        )));
    }
    let front = skills::parse(text)
        .filter(|front| front.name == name)
        .ok_or_else(|| {
            ToolError::msg(format!(
                "SKILL.md needs frontmatter with name {name:?} and a description"
            ))
        })?;
    if front.description.chars().count() > MAX_DESCRIPTION_CHARS {
        return Err(ToolError::msg(format!(
            "description is over {MAX_DESCRIPTION_CHARS} characters"
        )));
    }
    Ok(front)
}

fn check_support_size(len: usize) -> ToolResult<()> {
    if len > MAX_SUPPORT_BYTES {
        return Err(ToolError::msg(format!(
            "file is over {MAX_SUPPORT_BYTES} bytes"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use serde_json::{json, Value};

    use super::run;
    use crate::{read, ToolConfig, Toolbox};

    struct Env {
        _dir: tempfile::TempDir,
        base: PathBuf,
        global: PathBuf,
        project: PathBuf,
        tb: Toolbox,
    }

    fn env() -> Env {
        let dir = tempfile::tempdir().unwrap();
        let base = fs::canonicalize(dir.path()).unwrap();
        let global = base.join("global");
        let project = base.join("project");
        fs::create_dir_all(&global).unwrap();
        fs::create_dir_all(&project).unwrap();
        let tb = Toolbox::new(ToolConfig {
            global_skills: Some(global.clone()),
            ..ToolConfig::new(&project, base.join("spill"))
        })
        .unwrap();
        Env {
            _dir: dir,
            base,
            global,
            project,
            tb,
        }
    }

    /// A SKILL.md written by hand, as a human or a download would leave it.
    fn hand_skill(root: &Path, name: &str, origin: Option<&str>, body: &str) -> PathBuf {
        let dir = root.join(name);
        fs::create_dir_all(&dir).unwrap();
        let metadata = origin
            .map(|origin| format!("metadata:\n  origin: {origin}\n"))
            .unwrap_or_default();
        let text = format!(
            "---\nname: {name}\ndescription: test skill\n{metadata}---\n# {name}\n\n{body}\n"
        );
        let path = dir.join("SKILL.md");
        fs::write(&path, text).unwrap();
        path
    }

    fn agent_skill(root: &Path, name: &str, body: &str) -> PathBuf {
        hand_skill(root, name, Some("agent"), body)
    }

    fn skill_text(name: &str, description: &str, body: &str) -> String {
        format!("---\nname: {name}\ndescription: {description}\n---\n# {name}\n\n{body}\n")
    }

    fn create(name: &str, content: &str) -> Value {
        json!({"op": "create", "name": name, "content": content})
    }

    fn patch(name: &str, old: &str, new: &str) -> Value {
        json!({"op": "patch", "name": name, "old_string": old, "new_string": new})
    }

    fn write_file(name: &str, path: &str, content: &str) -> Value {
        json!({"op": "write_file", "name": name, "path": path, "content": content})
    }

    fn call(tb: &Toolbox, ops: Vec<Value>) -> Result<String, String> {
        run(tb, &json!({ "ops": ops })).map_err(|error| error.to_string())
    }

    /// Records the file as read, the way `read` does before a write is allowed.
    fn read_first(tb: &Toolbox, path: &Path) {
        read::run(tb, &json!({"path": path.display().to_string()})).unwrap();
    }

    #[test]
    fn create_forces_origin_agent_and_lands_in_global_by_default() {
        let e = env();
        let content = "---\nname: fresh\ndescription: new skill\nmetadata:\n  origin: human\n---\n# fresh\nbody\n";
        call(&e.tb, vec![create("fresh", content)]).unwrap();
        let text = fs::read_to_string(e.global.join("fresh/SKILL.md")).unwrap();
        assert!(text.contains("  origin: agent"), "{text}");
        assert!(!text.contains("origin: human"), "{text}");
    }

    #[test]
    fn create_takes_project_scope_and_refuses_a_name_already_in_either_root() {
        let e = env();
        let scoped = json!({"op": "create", "name": "here", "scope": "project",
            "content": skill_text("here", "project skill", "body")});
        call(&e.tb, vec![scoped]).unwrap();
        assert!(e.project.join(".agents/skills/here/SKILL.md").is_file());
        assert!(call(
            &e.tb,
            vec![create("here", &skill_text("here", "again", "b"))]
        )
        .is_err());
    }

    #[test]
    fn patch_and_write_refuse_human_and_downloaded_skills_and_leave_them_byte_identical() {
        let e = env();
        for (name, origin) in [("human-made", None), ("fetched", Some("downloaded"))] {
            let path = hand_skill(&e.global, name, origin, "keep me");
            read_first(&e.tb, &path);
            let before = fs::read(&path).unwrap();
            let err = call(&e.tb, vec![patch(name, "keep me", "changed")]).unwrap_err();
            assert!(err.contains("origin"), "{err}");
            let err = call(&e.tb, vec![write_file(name, "references/x.md", "x")]).unwrap_err();
            assert!(err.contains("origin"), "{err}");
            assert_eq!(fs::read(&path).unwrap(), before);
        }
    }

    #[test]
    fn patch_needs_exactly_one_match_unless_replace_all() {
        let e = env();
        let path = agent_skill(&e.global, "twice", "word word");
        read_first(&e.tb, &path);
        assert!(call(&e.tb, vec![patch("twice", "absent", "x")]).is_err());
        let err = call(&e.tb, vec![patch("twice", "word", "x")]).unwrap_err();
        assert!(err.contains("matches"), "{err}");
        let all = json!({"op": "patch", "name": "twice", "old_string": "word",
            "new_string": "x", "replace_all": true});
        call(&e.tb, vec![all]).unwrap();
        assert!(fs::read_to_string(&path).unwrap().contains("x x"));
    }

    #[test]
    fn support_paths_stay_inside_the_four_folders_and_the_skill() {
        let e = env();
        let path = agent_skill(&e.global, "guarded", "body");
        read_first(&e.tb, &path);
        for bad in [
            "../escape.txt",
            "/tmp/absolute.txt",
            "notes/x.md",
            "references/../../escape.txt",
            "SKILL.md/../../escape.txt",
        ] {
            assert!(
                call(&e.tb, vec![write_file("guarded", bad, "x")]).is_err(),
                "{bad}"
            );
        }
        assert!(!e.global.join("escape.txt").exists());
        let outside = e.base.join("outside");
        fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, e.global.join("guarded/references")).unwrap();
        assert!(call(&e.tb, vec![write_file("guarded", "references/x.md", "x")]).is_err());
        assert!(!outside.join("x.md").exists());
    }

    #[test]
    fn name_description_and_size_limits_refuse_before_anything_is_written() {
        let e = env();
        assert!(call(
            &e.tb,
            vec![create("Bad Name", &skill_text("Bad Name", "d", "b"))]
        )
        .is_err());
        let long = skill_text("long", &"d".repeat(1025), "body");
        assert!(call(&e.tb, vec![create("long", &long)]).is_err());
        assert!(!e.global.join("long").exists());
        let huge = skill_text("huge", "d", &"x".repeat(100_001));
        assert!(call(&e.tb, vec![create("huge", &huge)]).is_err());
        assert!(!e.global.join("huge").exists());
        let path = agent_skill(&e.global, "bulk", "body");
        read_first(&e.tb, &path);
        let big = "y".repeat((1 << 20) + 1);
        assert!(call(&e.tb, vec![write_file("bulk", "references/big.md", &big)]).is_err());
        assert!(!e.global.join("bulk/references/big.md").exists());
    }

    #[test]
    fn a_failing_op_rolls_back_every_op_before_it() {
        let e = env();
        let path = agent_skill(&e.global, "first", "before");
        read_first(&e.tb, &path);
        let before = fs::read(&path).unwrap();
        let err = call(
            &e.tb,
            vec![
                patch("first", "before", "after"),
                create("second", &skill_text("second", "new", "new body")),
                patch("first", "not there", "x"),
            ],
        );
        assert!(err.is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
        assert!(!e.global.join("second").exists());
    }

    #[test]
    fn concurrent_patches_both_land_or_one_is_refused_never_torn() {
        let e = env();
        let path = agent_skill(&e.global, "shared", "alpha beta");
        read_first(&e.tb, &path);
        let (a, b) = std::thread::scope(|scope| {
            let a = scope.spawn(|| call(&e.tb, vec![patch("shared", "alpha", "ALPHA")]));
            let b = scope.spawn(|| call(&e.tb, vec![patch("shared", "beta", "BETA")]));
            (a.join().unwrap(), b.join().unwrap())
        });
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("---\nname: shared\n"), "{text}");
        assert_eq!(a.is_ok(), text.contains("ALPHA"), "{text}");
        assert_eq!(b.is_ok(), text.contains("BETA"), "{text}");
        assert!(a.is_ok() || b.is_ok(), "{text}");
    }

    #[test]
    fn every_change_backs_up_the_previous_file_and_keeps_the_last_ten() {
        let e = env();
        let path = agent_skill(&e.global, "kept", "v0 marker");
        read_first(&e.tb, &path);
        for n in 1..=12 {
            let old = format!("v{}", n - 1);
            let new = format!("v{n}");
            call(&e.tb, vec![patch("kept", &old, &new)]).unwrap();
        }
        let backups = fs::read_dir(e.global.join(".backup/kept")).unwrap().count();
        assert_eq!(backups, 10);
    }

    #[test]
    fn patch_without_a_prior_read_is_refused_until_the_file_is_read() {
        let e = env();
        let path = agent_skill(&e.global, "unread", "old text");
        let err = call(&e.tb, vec![patch("unread", "old text", "new text")]).unwrap_err();
        let canonical = fs::canonicalize(&path).unwrap();
        assert!(
            err.contains(&format!(
                "Read {} with read before changing it.",
                canonical.display()
            )),
            "{err}"
        );
        read_first(&e.tb, &path);
        call(&e.tb, vec![patch("unread", "old text", "new text")]).unwrap();
        assert!(fs::read_to_string(&path).unwrap().contains("new text"));
    }

    #[test]
    fn each_call_appends_a_usage_line_and_a_read_of_a_skill_logs_a_view() {
        let e = env();
        let path = agent_skill(&e.global, "logged", "body");
        read_first(&e.tb, &path);
        call(&e.tb, vec![patch("logged", "body", "text")]).unwrap();
        assert!(call(&e.tb, vec![patch("logged", "nothing", "x")]).is_err());
        let log = fs::read_to_string(e.global.join(".usage.jsonl")).unwrap();
        let lines: Vec<Value> = log
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let ops: Vec<&str> = lines
            .iter()
            .map(|line| line["op"].as_str().unwrap())
            .collect();
        assert_eq!(ops, vec!["view", "patch", "patch"]);
        assert_eq!(lines[0]["name"], "logged");
        assert_eq!(lines[1]["result"], "ok");
        assert_ne!(lines[2]["result"], "ok");
    }

    #[test]
    fn remove_file_backs_up_a_supporting_file_and_never_removes_skill_md() {
        let e = env();
        agent_skill(&e.global, "tidy", "body");
        call(&e.tb, vec![write_file("tidy", "scripts/run.sh", "echo hi")]).unwrap();
        let remove = json!({"op": "remove_file", "name": "tidy", "path": "scripts/run.sh"});
        call(&e.tb, vec![remove]).unwrap();
        assert!(!e.global.join("tidy/scripts/run.sh").exists());
        assert!(e.global.join(".backup/tidy").is_dir());
        let skill_md = json!({"op": "remove_file", "name": "tidy", "path": "SKILL.md"});
        assert!(call(&e.tb, vec![skill_md]).is_err());
        assert!(e.global.join("tidy/SKILL.md").is_file());
    }
}
