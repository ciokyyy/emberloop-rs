//! Skill path grammar, identity and write rules for `read`, `edit` and `write`.
//!
//! `el://skills/<root>/<name>/<rel>` names a file under a skill root; the short form
//! `el://skills/<name>/<rel>` only reads, resolving the project root first. Any other path is a
//! disk path and takes no skill rules.

use std::fs;
use std::path::{Path, PathBuf};

use crate::skills::{self, Front};
use crate::skillstore;
use crate::tag::content_hash;
use crate::{ToolError, ToolResult, Toolbox};

/// `SKILL.md` at most this many characters.
const MAX_SKILL_CHARS: usize = 100_000;
/// One supporting file at most this many bytes.
const MAX_SUPPORT_BYTES: usize = 1 << 20;
/// A skill description at most this many characters.
const MAX_DESCRIPTION_CHARS: usize = 1024;

/// Which root a skill lives in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Root {
    Global,
    Project,
}

impl Root {
    fn label(self) -> &'static str {
        match self {
            Root::Global => "global",
            Root::Project => "project",
        }
    }
}

/// A parsed skill path. `root: None` is the read-only short form.
#[derive(Debug, PartialEq, Eq)]
pub enum SkillPath {
    Index,
    Item {
        root: Option<Root>,
        name: String,
        rel: String,
    },
}

/// What `parse` accepted: bare `el://skills`, or one skill.
pub fn parse(raw: &str) -> Option<SkillPath> {
    let raw = raw.trim();
    let rest = if let Some(rest) = raw.strip_prefix("el://skills") {
        rest
    } else if let Some(rest) = raw.strip_prefix("skills://") {
        rest
    } else {
        let rest = raw.strip_prefix("skill:")?;
        rest.strip_prefix("//").unwrap_or(rest)
    };
    let rest = rest.strip_prefix('/').unwrap_or(rest).trim_matches('/');
    if rest.is_empty() {
        return Some(SkillPath::Index);
    }
    let (first, tail) = match rest.split_once('/') {
        Some((first, tail)) => (first, tail),
        None => (rest, ""),
    };
    let root = match first {
        "global" => Some(Root::Global),
        "project" => Some(Root::Project),
        _ => None,
    };
    let (name, rel) = match root {
        Some(_) => match tail.split_once('/') {
            Some((name, rel)) => (name, rel),
            None => (tail, ""),
        },
        None => (first, tail),
    };
    if name.is_empty() {
        return None;
    }
    let rel = if rel.trim_matches('/').is_empty() {
        "SKILL.md".to_string()
    } else {
        rel.trim_matches('/').to_string()
    };
    Some(SkillPath::Item {
        root,
        name: name.to_string(),
        rel,
    })
}

/// The root-named form a write must use.
fn root_named_required() -> ToolError {
    ToolError::msg(
        "change a skill through its root-named path, el://skills/global/<name>/... or \
         el://skills/project/<name>/...",
    )
}

/// A file under a skill root: `path` keeps the root spelling for the root's own bookkeeping, `key`
/// is the canonical disk path the seen store uses.
#[derive(Debug, Clone)]
pub struct Resolved {
    pub root: Root,
    pub name: String,
    pub rel: String,
    pub path: PathBuf,
    pub key: PathBuf,
}

fn root_dir(toolbox: &Toolbox, root: Root) -> ToolResult<PathBuf> {
    match root {
        Root::Project => Ok(toolbox.root().join(skills::PROJECT_SKILLS)),
        Root::Global => toolbox
            .global_skills()
            .map(Path::to_path_buf)
            .ok_or_else(|| ToolError::msg("this Session has no global skills root")),
    }
}

/// Resolves a parsed path. The short form picks the project root first, then global.
pub fn resolve(toolbox: &Toolbox, target: &SkillPath) -> ToolResult<Resolved> {
    let SkillPath::Item { root, name, rel } = target else {
        return Err(ToolError::msg(
            "el://skills is the index; read el://skills/<name> instead",
        ));
    };
    if !skills::valid_name(name) {
        return Err(ToolError::msg(format!(
            "{name:?} is not a valid skill name"
        )));
    }
    let root = match root {
        Some(root) => *root,
        None => short_root(toolbox, name)?,
    };
    let dir = root_dir(toolbox, root)?.join(name);
    let path = safe_rel(&dir, rel)?;
    let key = canonical_key(&path)?;
    Ok(Resolved {
        root,
        name: name.clone(),
        rel: rel.clone(),
        path,
        key,
    })
}

fn short_root(toolbox: &Toolbox, name: &str) -> ToolResult<Root> {
    let project = toolbox
        .root()
        .join(skills::PROJECT_SKILLS)
        .join(name)
        .join("SKILL.md");
    if project.is_file() {
        return Ok(Root::Project);
    }
    if toolbox
        .global_skills()
        .is_some_and(|global| global.join(name).join("SKILL.md").is_file())
    {
        return Ok(Root::Global);
    }
    Err(ToolError::msg(format!("skill {name:?} was not found")))
}

/// The canonical path, walking up to the deepest folder that exists so a create still has a key.
fn canonical_key(path: &Path) -> ToolResult<PathBuf> {
    let mut ancestor = path;
    let mut missing: Vec<&std::ffi::OsStr> = Vec::new();
    loop {
        if let Ok(base) = fs::canonicalize(ancestor) {
            let mut key = base;
            for part in missing.iter().rev() {
                key.push(part);
            }
            return Ok(key);
        }
        match (ancestor.parent(), ancestor.file_name()) {
            (Some(parent), Some(name)) => {
                missing.push(name);
                ancestor = parent;
            }
            _ => return Ok(path.to_path_buf()),
        }
    }
}

/// A relative file under a skill: no `..`, no leading `/`, and no symlink on the way.
fn safe_rel(dir: &Path, rel: &str) -> ToolResult<PathBuf> {
    let mut target = dir.to_path_buf();
    for comp in Path::new(rel).components() {
        match comp {
            std::path::Component::Normal(part) => {
                target.push(part);
                if target
                    .symlink_metadata()
                    .is_ok_and(|meta| meta.file_type().is_symlink())
                {
                    return Err(ToolError::msg(format!(
                        "{} is a symlink; skill files must stay inside the skill",
                        target.display()
                    )));
                }
            }
            _ => {
                return Err(ToolError::msg(format!(
                    "path {rel:?} must be relative, with no .. and no leading /"
                )))
            }
        }
    }
    Ok(target)
}

/// The root-named path shown in replies.
pub fn display(resolved: &Resolved) -> String {
    format!(
        "el://skills/{}/{}/{}",
        resolved.root.label(),
        resolved.name,
        resolved.rel
    )
}

/// The change a write makes, which also picks the `.usage.jsonl` op.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Create,
    Patch,
    WriteFile,
}

impl Op {
    fn label(self) -> &'static str {
        match self {
            Op::Create => "create",
            Op::Patch => "patch",
            Op::WriteFile => "write_file",
        }
    }
}

/// An edit or write target: `Some` for a root-named skill path, `None` for a disk path, and an
/// error for the short form and every other `el://`, `skills://` or `skill:` path.
pub fn writable(toolbox: &Toolbox, raw: &str) -> ToolResult<Option<Resolved>> {
    let Some(target) = parse(raw) else {
        return if ["el://", "skills://", "skill:"]
            .iter()
            .any(|scheme| raw.trim().starts_with(scheme))
        {
            Err(root_named_required())
        } else {
            Ok(None)
        };
    };
    match target {
        SkillPath::Item { root: Some(_), .. } => resolve(toolbox, &target).map(Some),
        _ => Err(root_named_required()),
    }
}

/// The rules a write must satisfy before anything is touched, and the op it makes. The
/// read-before-write rule is the caller's: `write` asks `require_seen`, `edit` has already
/// validated the anchor snapshot it applies to.
pub fn check_write(toolbox: &Toolbox, resolved: &Resolved, new_text: &str) -> ToolResult<Op> {
    let exists = resolved.path.is_file();
    let dir = root_dir(toolbox, resolved.root)?.join(&resolved.name);
    if resolved.rel != "SKILL.md" {
        skillstore::support_path(&dir, &resolved.rel)?;
        check_support_size(new_text.len())?;
        let text = fs::read_to_string(dir.join("SKILL.md")).map_err(|_| {
            ToolError::msg(format!(
                "skill {} has no SKILL.md; write that first",
                resolved.name
            ))
        })?;
        require_agent(&resolved.name, &text)?;
        return Ok(Op::WriteFile);
    }
    if exists {
        let front = check_skill_text(&resolved.name, new_text)?;
        let before = fs::read_to_string(&resolved.path)?;
        require_agent(&resolved.name, &before)?;
        if front.origin != "agent" {
            return Err(ToolError::msg(
                "the change must keep metadata.origin: agent",
            ));
        }
        return Ok(Op::Patch);
    }
    if matches!(resolved.name.as_str(), "global" | "project") {
        return Err(ToolError::msg(format!(
            "{:?} is a reserved name; the first path segment is the skill root",
            resolved.name
        )));
    }
    for root in [Root::Global, Root::Project] {
        if root_dir(toolbox, root)
            .is_ok_and(|dir| dir.join(&resolved.name).join("SKILL.md").is_file())
        {
            return Err(ToolError::msg(format!(
                "skill {:?} already exists in the {} root",
                resolved.name,
                root.label()
            )));
        }
    }
    check_skill_text(&resolved.name, new_text)?;
    Ok(Op::Create)
}

/// Locks the skill, backs up `old`, writes `new` and appends its usage line. A create inserts
/// `metadata.origin: agent` when the frontmatter has none. The caller records the seen snapshot
/// of `new`, because only it knows the lines the model was shown.
pub fn commit(
    toolbox: &Toolbox,
    resolved: &Resolved,
    op: Op,
    old: Option<&[u8]>,
    new: &[u8],
) -> ToolResult<()> {
    let root = root_dir(toolbox, resolved.root)?;
    let _lock = skillstore::lock(&root, &resolved.name)?;
    let bytes = match op {
        Op::Create => {
            let text = std::str::from_utf8(new)
                .map_err(|_| ToolError::msg("SKILL.md must be UTF-8 text"))?;
            skills::with_agent_origin(text)
                .ok_or_else(|| ToolError::msg("SKILL.md must open with --- frontmatter"))?
                .into_bytes()
        }
        _ => new.to_vec(),
    };
    if let Some(old) = old {
        let flat = resolved.rel.replace('/', "__");
        skillstore::backup(&root, &resolved.name, &flat, old)?;
    }
    if let Some(parent) = resolved.path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&resolved.path, &bytes)?;
    skillstore::log_usage(&root, &resolved.name, op.label(), "ok")?;
    Ok(())
}

/// Logs one `view` line for a skill read through a skill path. The root spelling is used as
/// requested, so a symlinked skill logs under the root it is linked from.
pub fn log_view(toolbox: &Toolbox, resolved: &Resolved) {
    if resolved.rel != "SKILL.md" {
        return;
    }
    let Ok(root) = root_dir(toolbox, resolved.root) else {
        return;
    };
    if let Err(error) = skillstore::log_usage(&root, &resolved.name, "view", "ok") {
        tracing::warn!(%error, "could not log the skill view");
    }
}

/// `SKILL.md` must be under the size limit, name the skill, and carry a description of at most
/// `MAX_DESCRIPTION_CHARS` characters.
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

/// An existing skill must be agent-origin to change; a missing origin means `human`.
fn require_agent(name: &str, text: &str) -> ToolResult<()> {
    let origin = skills::parse(text).map_or_else(|| "human".to_string(), |front| front.origin);
    if origin == "agent" {
        return Ok(());
    }
    Err(ToolError::msg(format!(
        "skill {name} has origin {origin}; only agent-origin skills change"
    )))
}

/// This Session must have read exactly the current bytes of the file.
pub fn require_seen(toolbox: &Toolbox, resolved: &Resolved) -> ToolResult<()> {
    let bytes = fs::read(&resolved.path)?;
    if toolbox
        .seen()
        .unseen(&resolved.key, &content_hash(&bytes), &[])
        .is_none()
    {
        return Err(ToolError::msg(format!(
            "Read {} with read before changing it.",
            display(resolved)
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_norms_the_root_the_aliases_and_the_default_rel() {
        assert_eq!(
            parse("el://skills/global/ember-flow"),
            Some(SkillPath::Item {
                root: Some(Root::Global),
                name: "ember-flow".to_string(),
                rel: "SKILL.md".to_string()
            })
        );
        assert_eq!(
            parse("skills://project/flow/references/a.md/"),
            Some(SkillPath::Item {
                root: Some(Root::Project),
                name: "flow".to_string(),
                rel: "references/a.md".to_string()
            })
        );
        assert_eq!(
            parse("skill://flow"),
            Some(SkillPath::Item {
                root: None,
                name: "flow".to_string(),
                rel: "SKILL.md".to_string()
            })
        );
        assert_eq!(parse("el://skills"), Some(SkillPath::Index));
        assert_eq!(parse("el://docs/config"), None);
        assert_eq!(parse("/tmp/skill/SKILL.md"), None);
    }

    #[test]
    fn a_bare_root_is_not_a_skill_and_a_skill_named_like_one_keeps_the_root() {
        assert_eq!(parse("el://skills/global"), None);
        assert_eq!(
            parse("el://skills/global/global/SKILL.md"),
            Some(SkillPath::Item {
                root: Some(Root::Global),
                name: "global".to_string(),
                rel: "SKILL.md".to_string()
            })
        );
    }

    fn toolbox(root: &Path) -> Toolbox {
        for dir in ["project", "spill", "skills"] {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        let mut config = crate::ToolConfig::new(root.join("project"), root.join("spill"));
        config.global_skills = Some(root.join("skills"));
        Toolbox::new(config).unwrap()
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_skill_keeps_its_root_spelling_apart_from_its_canonical_key() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real/linked");
        fs::create_dir_all(&real).unwrap();
        fs::write(
            real.join("SKILL.md"),
            "---\nname: linked\ndescription: x\n---\n",
        )
        .unwrap();
        fs::create_dir_all(tmp.path().join("skills")).unwrap();
        std::os::unix::fs::symlink(&real, tmp.path().join("skills/linked")).unwrap();
        let toolbox = toolbox(tmp.path());
        let resolved = resolve(&toolbox, &parse("el://skills/global/linked").unwrap()).unwrap();
        assert_eq!(resolved.path, tmp.path().join("skills/linked/SKILL.md"));
        assert_eq!(
            resolved.key,
            fs::canonicalize(&real).unwrap().join("SKILL.md")
        );
    }

    #[test]
    fn a_new_skill_may_not_take_a_root_name() {
        let tmp = tempfile::tempdir().unwrap();
        let toolbox = toolbox(tmp.path());
        let resolved = resolve(&toolbox, &parse("el://skills/project/project").unwrap()).unwrap();
        let error = check_write(
            &toolbox,
            &resolved,
            "---\nname: project\ndescription: x\n---\n",
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("reserved name"), "got: {error}");
    }

    #[test]
    fn only_skill_paths_take_the_skill_rules() {
        let tmp = tempfile::tempdir().unwrap();
        let toolbox = toolbox(tmp.path());
        for path in [
            "el://docs/config",
            "skills://docs/config",
            "skill:docs/config",
        ] {
            let error = writable(&toolbox, path).unwrap_err().to_string();
            assert!(
                error.contains("el://skills/global/<name>/..."),
                "{path}: {error}"
            );
        }
        assert!(writable(&toolbox, "/tmp/notes.md").unwrap().is_none());
        assert!(writable(&toolbox, "el://skills/global/flow")
            .unwrap()
            .is_some());
    }
}
