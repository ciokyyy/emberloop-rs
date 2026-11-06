use std::fs;
use std::path::{Path, PathBuf};

use glob::Pattern;
use ignore::WalkBuilder;
use regex::{Regex, RegexBuilder};
use serde::Deserialize;
use serde_json::Value;

use crate::tag::{content_hash, lines_of};
use crate::{ToolError, ToolResult, Toolbox};

const DEFAULT_MAX_RESULTS: usize = 100;

#[derive(Deserialize)]
struct GrepArgs {
    pattern: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    glob: Option<String>,
    #[serde(default)]
    case_insensitive: bool,
    #[serde(default)]
    context: usize,
    #[serde(default)]
    max_results: Option<usize>,
    /// `content` (default), `files` or `count`.
    #[serde(default)]
    output_mode: Option<String>,
}

struct FileHits {
    path: PathBuf,
    display: String,
    hash: String,
    lines: Vec<String>,
    matches: Vec<usize>,
}

pub fn run(toolbox: &Toolbox, args: &Value) -> ToolResult<String> {
    let args: GrepArgs = serde_json::from_value(args.clone())?;
    if args.pattern.is_empty() {
        return Err(ToolError::msg("pattern is required"));
    }
    let regex = RegexBuilder::new(&args.pattern)
        .case_insensitive(args.case_insensitive)
        .build()
        .map_err(|e| ToolError::msg(format!("invalid pattern: {e}")))?;
    let glob = args
        .glob
        .as_deref()
        .map(Pattern::new)
        .transpose()
        .map_err(|e| ToolError::msg(format!("invalid glob: {e}")))?;
    let base = match &args.path {
        Some(path) => toolbox.jail().resolve(path)?,
        None => toolbox.root().to_path_buf(),
    };
    let limit = args.max_results.unwrap_or(DEFAULT_MAX_RESULTS).max(1);
    let files = collect_files(&base, glob.as_ref())?;
    let mut hits = Vec::new();
    let mut total = 0;
    for file in files {
        if total >= limit {
            break;
        }
        let Some(file_hits) = scan_file(&file, toolbox.root(), &regex, limit - total) else {
            continue;
        };
        total += file_hits.matches.len();
        if !file_hits.matches.is_empty() {
            let context = content_context(&args);
            record_seen(&file_hits, context, toolbox.seen());
            hits.push(file_hits);
        }
    }
    let rendered = render(&args, &hits)?;
    toolbox.maybe_spill(rendered, &format!("grep {:?}", args.pattern))
}

fn content_context(args: &GrepArgs) -> usize {
    if args.output_mode.as_deref().unwrap_or("content") == "content" {
        args.context
    } else {
        0
    }
}

fn collect_files(base: &Path, glob: Option<&Pattern>) -> ToolResult<Vec<PathBuf>> {
    if base.is_file() {
        return Ok(vec![base.to_path_buf()]);
    }
    if !base.is_dir() {
        return Err(ToolError::msg(format!(
            "{} is not a file or directory",
            base.display()
        )));
    }
    let walker = WalkBuilder::new(base)
        .require_git(false)
        .sort_by_file_path(|a, b| a.cmp(b))
        .build();
    let mut files = Vec::new();
    for entry in walker.flatten() {
        let path = entry.path();
        if path.is_file() && glob.is_none_or(|pattern| glob_matches(pattern, path, base)) {
            files.push(path.to_path_buf());
        }
    }
    Ok(files)
}

fn glob_matches(pattern: &Pattern, path: &Path, base: &Path) -> bool {
    let target = match path.strip_prefix(base) {
        Ok(relative) => relative.to_string_lossy(),
        Err(_) => path.to_string_lossy(),
    };
    if pattern.as_str().contains('/') {
        pattern.matches(&target)
    } else {
        path.file_name()
            .map(|name| pattern.matches(&name.to_string_lossy()))
            .unwrap_or(false)
    }
}

fn scan_file(path: &Path, root: &Path, regex: &Regex, limit: usize) -> Option<FileHits> {
    let bytes = fs::read(path).ok()?;
    if bytes.contains(&0) {
        return None;
    }
    let text = String::from_utf8(bytes.clone()).ok()?;
    let lines: Vec<String> = lines_of(&text).into_iter().map(str::to_string).collect();
    let mut matches = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if regex.is_match(line) {
            matches.push(index + 1);
            if matches.len() >= limit {
                break;
            }
        }
    }
    Some(FileHits {
        path: path.to_path_buf(),
        display: display_path(path, root),
        hash: content_hash(&bytes),
        lines,
        matches,
    })
}

fn render(args: &GrepArgs, hits: &[FileHits]) -> ToolResult<String> {
    if hits.is_empty() {
        return Ok(format!("no matches for {:?}\n", args.pattern));
    }
    match args.output_mode.as_deref().unwrap_or("content") {
        "files" => Ok(render_files(hits)),
        "count" => Ok(render_counts(hits)),
        "content" => Ok(render_content(args.context, hits)),
        other => Err(ToolError::msg(format!(
            "unknown output_mode {other:?}; use content, files or count"
        ))),
    }
}

fn render_files(hits: &[FileHits]) -> String {
    let mut out = String::new();
    for hit in hits {
        out.push_str(&hit.display);
        out.push('\n');
    }
    out
}

fn render_counts(hits: &[FileHits]) -> String {
    let mut out = String::new();
    for hit in hits {
        out.push_str(&format!("{}:{}\n", hit.display, hit.matches.len()));
    }
    out
}

fn render_content(context: usize, hits: &[FileHits]) -> String {
    let mut out = String::new();
    for hit in hits {
        for (group_index, (start, end)) in ranges(&hit.matches, context, hit.lines.len())
            .into_iter()
            .enumerate()
        {
            if group_index > 0 {
                out.push_str("--\n");
            }
            for line_no in start..=end {
                out.push_str(&format!(
                    "{}:{}:{}\n",
                    hit.display,
                    line_no,
                    hit.lines[line_no - 1]
                ));
            }
        }
    }
    out
}

/// Merges per-match context windows into disjoint 1-based line ranges.
fn ranges(matches: &[usize], context: usize, line_count: usize) -> Vec<(usize, usize)> {
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for &line in matches {
        let start = line.saturating_sub(context).max(1);
        let end = (line + context).min(line_count);
        match merged.last_mut() {
            Some(last) if start <= last.1 + 1 => last.1 = last.1.max(end),
            _ => merged.push((start, end)),
        }
    }
    merged
}

fn record_seen(hit: &FileHits, context: usize, seen: &crate::SeenLines) {
    let mut visible = Vec::new();
    for &line in &hit.matches {
        let start = line.saturating_sub(context).max(1) as u32;
        let end = (line + context).min(hit.lines.len()) as u32;
        visible.extend(start..=end);
    }
    seen.record_with_content(&hit.path, &hit.hash, hit.lines.clone(), visible, false);
}

fn display_path(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .to_string()
}
