use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

use crate::tag::{content_hash, lines_of};
use crate::{skills, skillstore, ToolError, ToolResult, Toolbox};

#[derive(Deserialize)]
struct ReadArgs {
    path: String,
    #[serde(default)]
    offset: usize,
    #[serde(default)]
    limit: usize,
}

/// `ctx:<handle>` optionally followed by `:<start>-<end>`, `:<line>` or `:<line>+<count>`.
fn parse_ctx(raw: &str) -> Option<(&str, Option<&str>)> {
    let rest = raw.strip_prefix("ctx:")?;
    match rest.split_once(':') {
        Some((handle, range)) if !handle.is_empty() => Some((handle, Some(range))),
        Some(_) => None,
        None if rest.is_empty() => None,
        None => Some((rest, None)),
    }
}

fn parse_ctx_range(range: &str) -> Option<(usize, usize)> {
    if let Some((start, end)) = range.split_once('-') {
        let start = start.parse::<usize>().ok()?;
        let end = end.parse::<usize>().ok()?;
        return Some((start, end.max(start)));
    }
    if let Some((start, extra)) = range.split_once('+') {
        let start = start.parse::<usize>().ok()?;
        let extra = extra.parse::<usize>().ok()?;
        return Some((start, start + extra));
    }
    let line = range.parse::<usize>().ok()?;
    Some((line, line))
}

pub fn run(toolbox: &Toolbox, args: &Value) -> ToolResult<String> {
    let args: ReadArgs = serde_json::from_value(args.clone())?;
    if let Some((handle, range)) = parse_ctx(&args.path) {
        return read_ctx(toolbox, handle, range, args.offset, args.limit);
    }
    if let Some(target) = parse_skill_target(&args.path) {
        return read_skill(toolbox, target, args.offset, args.limit);
    }
    let path = toolbox_jail(toolbox).resolve(&args.path)?;
    let rendered = if path.is_dir() {
        render_dir(&path, args.offset, args.limit)?
    } else {
        let rendered = render_file(toolbox, &path, args.offset, args.limit)?;
        skillstore::log_view(toolbox, &args.path);
        rendered
    };
    toolbox.maybe_spill(rendered, &format!("read {}", args.path))
}

fn toolbox_jail(toolbox: &Toolbox) -> &crate::ProjectJail {
    &toolbox.jail
}

fn read_ctx(
    toolbox: &Toolbox,
    handle: &str,
    range: Option<&str>,
    offset: usize,
    limit: usize,
) -> ToolResult<String> {
    let path = toolbox
        .overflow
        .path(handle)
        .ok_or_else(|| ToolError::msg(format!("unknown ctx handle {handle:?}")))?;
    let text = fs::read_to_string(&path)?;
    let lines = lines_of(&text);
    let (start, end) = match range {
        Some(range) => {
            let (start, end) = parse_ctx_range(range).ok_or_else(|| {
                ToolError::msg(format!("invalid ctx range {range:?}; use N, N-M or N+M"))
            })?;
            window(&lines, start, end.saturating_sub(start) + 1)
        }
        None => window(&lines, offset, limit),
    };
    Ok(render_window(
        &format!("ctx:{handle}"),
        &lines,
        start,
        end,
        false,
    ))
}
#[derive(Debug, PartialEq, Eq)]
enum SkillTarget<'a> {
    Catalog,
    Item {
        name: &'a str,
        subpath: Option<&'a str>,
    },
}

fn parse_skill_target(raw: &str) -> Option<SkillTarget<'_>> {
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
        return Some(SkillTarget::Catalog);
    }
    let (name, subpath) = match rest.split_once('/') {
        Some((name, sub)) => {
            let sub = sub.trim_matches('/');
            let sub = if sub.is_empty() || sub == "SKILL.md" {
                None
            } else {
                Some(sub)
            };
            (name, sub)
        }
        None => (rest, None),
    };
    Some(SkillTarget::Item { name, subpath })
}

fn safe_skill_subpath(dir: &Path, rel: &str) -> ToolResult<PathBuf> {
    let mut target = dir.to_path_buf();
    for comp in Path::new(rel).components() {
        match comp {
            std::path::Component::Normal(part) => {
                target.push(part);
                if target
                    .symlink_metadata()
                    .is_ok_and(|m| m.file_type().is_symlink())
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
                )));
            }
        }
    }
    Ok(target)
}

fn read_skill(
    toolbox: &Toolbox,
    target: SkillTarget<'_>,
    offset: usize,
    limit: usize,
) -> ToolResult<String> {
    match target {
        SkillTarget::Catalog => {
            let skills = skills::discover(toolbox.global_skills(), toolbox.root());
            if skills.is_empty() {
                return toolbox.maybe_spill("No skills found.\n".to_string(), "read el://skills");
            }
            let mut out = String::from("Available skills:\n\n");
            for skill in skills {
                let scope = if skill.project { "project" } else { "global" };
                out.push_str(&format!(
                    "- {} [{}] [{}]: {}\n",
                    skill.name, skill.origin, scope, skill.description
                ));
            }
            toolbox.maybe_spill(out, "read el://skills")
        }
        SkillTarget::Item { name, subpath } => {
            let skill = skills::locate(toolbox.global_skills(), toolbox.root(), name)
                .ok_or_else(|| ToolError::msg(format!("skill {name:?} was not found")))?;
            let skill_dir = skill.path.parent().ok_or_else(|| {
                ToolError::msg(format!("invalid skill path {}", skill.path.display()))
            })?;
            let target_path = match subpath {
                Some(sub) => safe_skill_subpath(skill_dir, sub)?,
                None => skill.path.clone(),
            };
            let rendered = if target_path.is_dir() {
                render_dir(&target_path, offset, limit)?
            } else {
                let rendered = render_file(toolbox, &target_path, offset, limit)?;
                skillstore::log_view(toolbox, &target_path.display().to_string());
                rendered
            };
            let label = match subpath {
                Some(sub) => format!("read el://skills/{name}/{sub}"),
                None => format!("read el://skills/{name}"),
            };
            toolbox.maybe_spill(rendered, &label)
        }
    }
}

fn render_file(toolbox: &Toolbox, path: &Path, offset: usize, limit: usize) -> ToolResult<String> {
    let meta = fs::metadata(path)?;
    if !meta.is_file() {
        return Err(ToolError::msg(format!(
            "{} is not a regular file",
            path.display()
        )));
    }
    if meta.len() > toolbox.max_read_bytes() as u64 {
        return Err(ToolError::msg(format!(
            "{} is {} bytes, over the {} byte read limit; search it with grep or print a range with bash `sed -n 'START,ENDp'`",
            path.display(),
            meta.len(),
            toolbox.max_read_bytes()
        )));
    }
    let bytes = fs::read(path)?;
    let text = String::from_utf8(bytes.clone()).map_err(|_| {
        ToolError::msg(format!(
            "{} is binary; use bash to inspect it",
            path.display()
        ))
    })?;
    if text.contains('\0') {
        return Err(ToolError::msg(format!(
            "{} is binary; use bash to inspect it",
            path.display()
        )));
    }
    let hash = content_hash(&bytes);
    let tag = &hash[..4];
    let lines = lines_of(&text);
    let (start, end) = window(&lines, offset, limit);
    let owned_lines: Vec<String> = lines.iter().map(|s| (*s).to_string()).collect();
    toolbox.seen().record_with_content(
        path,
        &hash,
        owned_lines,
        (start + 1) as u32..=end as u32,
        false,
    );
    Ok(render_window(
        &format!("{}#{}", path.display(), tag),
        &lines,
        start,
        end,
        true,
    ))
}

fn render_dir(path: &Path, offset: usize, limit: usize) -> ToolResult<String> {
    let mut entries = fs::read_dir(path)?
        .filter_map(|entry| entry.ok())
        .map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            let meta = entry.metadata().ok();
            match meta {
                Some(meta) if meta.is_dir() => format!("{name}/"),
                Some(meta) => format!("{name}  {} B", meta.len()),
                None => name,
            }
        })
        .collect::<Vec<_>>();
    entries.sort();
    let start = (offset.max(1) - 1).min(entries.len());
    let end = if limit == 0 {
        entries.len()
    } else {
        (start + limit).min(entries.len())
    };
    let mut out = format!("[{}] {} entries\n", path.display(), entries.len());
    for entry in &entries[start..end] {
        out.push_str(entry);
        out.push('\n');
    }
    Ok(out)
}

/// Turns 1-based offset/limit into slice bounds. `limit == 0` means the rest.
fn window(lines: &[&str], offset: usize, limit: usize) -> (usize, usize) {
    let start = (offset.max(1) - 1).min(lines.len());
    let end = if limit == 0 {
        lines.len()
    } else {
        (start + limit).min(lines.len())
    };
    (start, end)
}

fn render_window(label: &str, lines: &[&str], start: usize, end: usize, numbered: bool) -> String {
    let (first, last) = if end > start {
        (start + 1, end)
    } else {
        (0, 0)
    };
    let mut out = format!("[{label}] (lines {first}-{last})\n");
    for (index, line) in lines[start..end].iter().enumerate() {
        if numbered {
            out.push_str(&format!("{:>6}\u{2192}{line}\n", start + index + 1));
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

pub fn resolve(toolbox: &Toolbox, raw: &str) -> ToolResult<PathBuf> {
    toolbox_jail(toolbox).resolve(raw)
}
