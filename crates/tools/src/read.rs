use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

use crate::tag::{content_hash, lines_of};
use crate::{docs, skillpath, skills, skillstore, ToolError, ToolResult, Toolbox};

#[derive(Deserialize)]
struct ReadArgs {
    path: String,
    #[serde(default)]
    offset: usize,
    #[serde(default)]
    limit: usize,
}

/// Image file a `read` call names: MIME type and base64 data, so the model sees the picture.
/// `None` when the path is not an image extension; checked after the jail and size limit.
pub fn image(toolbox: &Toolbox, args: &Value) -> Option<ToolResult<(&'static str, String)>> {
    let raw = args.get("path")?.as_str()?;
    let mime = match raw.rsplit('.').next()?.to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => return None,
    };
    if !toolbox.image_input() {
        return Some(Err(ToolError::msg(format!(
            "{raw} is an image and the current model takes no image input"
        ))));
    }
    Some((|| {
        let path = toolbox.resolve_read(raw)?;
        let meta = fs::metadata(&path)?;
        if meta.len() > toolbox.max_read_bytes() as u64 {
            return Err(ToolError::msg(format!(
                "{} is {} bytes, over the {} byte read limit",
                path.display(),
                meta.len(),
                toolbox.max_read_bytes()
            )));
        }
        Ok((mime, base64(&fs::read(&path)?)))
    })())
}

fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | (*b as u32) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
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

/// Lines a plain read shows; `path:raw` asks for the whole file. Skills are never capped.
pub(crate) const DEFAULT_LINES: usize = 300;

/// Splits the `:raw` suffix off a path; the URL reader has the same suffix for its own raw mode.
fn split_raw(raw: &str) -> (String, bool) {
    match raw.strip_suffix(":raw") {
        Some(base) if !base.is_empty() => (base.to_string(), true),
        _ => (raw.to_string(), false),
    }
}

pub fn run(toolbox: &Toolbox, args: &Value) -> ToolResult<String> {
    let args: ReadArgs = serde_json::from_value(args.clone())?;
    let (path_arg, raw) = split_raw(&args.path);
    if let Some((handle, range)) = parse_ctx(&path_arg) {
        return read_ctx(toolbox, handle, range, args.offset, args.limit);
    }
    if let Some(target) = skillpath::parse(&path_arg) {
        return read_skill(toolbox, &target, args.offset, args.limit);
    }
    if let Some(target) = docs::parse_target(&path_arg) {
        return docs::read(toolbox, target);
    }
    let path = toolbox.resolve_read(&path_arg)?;
    if path.is_dir() {
        return render_dir(&path, args.offset, args.limit);
    }
    let limit = if raw {
        0
    } else if args.limit == 0 {
        DEFAULT_LINES
    } else {
        args.limit
    };
    let rendered = render_file(toolbox, &path, &path, None, args.offset, limit)?;
    skillstore::log_view(toolbox, &path_arg);
    // No spill: read answers with its own lines, and `:raw` is how a model asks for all of them.
    Ok(rendered)
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
fn read_skill(
    toolbox: &Toolbox,
    target: &skillpath::SkillPath,
    offset: usize,
    limit: usize,
) -> ToolResult<String> {
    if matches!(target, skillpath::SkillPath::Index) {
        let index = skills::index(toolbox.global_skills(), toolbox.root());
        if index.is_empty() {
            return Ok("No skills found.\n".to_string());
        }
        return Ok(index);
    }
    let resolved = skillpath::resolve(toolbox, target)?;
    if resolved.path.is_dir() {
        return render_dir(&resolved.path, offset, limit);
    }
    let text = fs::read_to_string(&resolved.path)?;
    let total = text.lines().count();
    let page = skill_page(&text, offset);
    let mut rendered = render_file(
        toolbox,
        &resolved.path,
        &resolved.key,
        Some(&skillpath::display(&resolved)),
        offset,
        page,
    )?;
    let last = (offset.max(1) - 1 + page).min(total);
    if last < total {
        rendered.push_str(&format!(
            "\n[skill continues at line {}: read again with offset {}]\n",
            last + 1,
            last + 1
        ));
    }
    skillpath::log_view(toolbox, &resolved);
    Ok(rendered)
}

/// Input bytes of a skill one tool result may carry. pi cuts a model-facing MCP result over 20 KB
/// (`crates/pi-mcp/src/output.rs`) and no setting raises it, so a skill pages instead of losing its
/// middle; the numbered output of this budget stays under that.
const SKILL_PAGE_BYTES: usize = 16 * 1024;

/// How many lines from `offset` fit one skill page, at least one.
fn skill_page(text: &str, offset: usize) -> usize {
    let mut bytes = 0;
    let mut lines = 0;
    // shortcut: one line over the budget still pages alone, `upgrade` if a model ever ships such a
    // skill and the harness cuts it.
    for line in text.lines().skip(offset.saturating_sub(1)) {
        bytes += line.len() + 1;
        if bytes > SKILL_PAGE_BYTES && lines > 0 {
            break;
        }
        lines += 1;
    }
    lines.max(1)
}

fn render_file(
    toolbox: &Toolbox,
    path: &Path,
    key: &Path,
    label: Option<&str>,
    offset: usize,
    limit: usize,
) -> ToolResult<String> {
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
        key,
        &hash,
        owned_lines,
        (start + 1) as u32..=end as u32,
        false,
    );
    let name = label.map_or_else(|| path.display().to_string(), str::to_string);
    Ok(render_window(
        &format!("{name}#{tag}"),
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

/// Any path `read` opens for this caller: `~` expands from `HOME`, a relative path joins the
/// project root, and a goldfish is confined to its allowlist.
pub fn resolve(toolbox: &Toolbox, raw: &str) -> ToolResult<PathBuf> {
    toolbox.resolve_read(raw)
}

#[cfg(test)]
mod image_tests {
    #[test]
    fn base64_pads() {
        assert_eq!(super::base64(b"Man"), "TWFu");
        assert_eq!(super::base64(b"Ma"), "TWE=");
        assert_eq!(super::base64(b"M"), "TQ==");
        assert_eq!(super::base64(b""), "");
    }
}
