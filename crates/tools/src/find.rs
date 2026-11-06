use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use ignore::WalkBuilder;
use serde::Deserialize;
use serde_json::Value;

use crate::tag::{content_hash, lines_of};
use crate::{ToolError, ToolResult, Toolbox};

const DEFAULT_HITS: usize = 12;
const MAX_HITS: usize = 50;
const MAX_BYTES: usize = 6 * 1024;
const THRESHOLD: f64 = 0.20;
const PASSAGE_WINDOW: usize = 6;

const STOPWORDS: &[&str] = &[
    "the", "and", "for", "with", "from", "that", "this", "where", "what", "which", "when", "does",
    "are", "how", "why", "had", "has", "was", "were", "been", "into", "over", "under", "about",
    "there", "their", "them", "then", "than", "also", "such", "some", "any", "all", "our", "your",
    "its", "can", "will", "a", "an", "is", "it", "of", "or", "to", "in", "on", "as", "by", "at",
    "be", "do", "if", "so",
];

const IGNORED_EXTS: &[&str] = &[
    ".lock",
    ".map",
    ".min.js",
    ".bundle.js",
    ".png",
    ".jpg",
    ".jpeg",
    ".gif",
    ".svg",
    ".ico",
    ".woff",
    ".woff2",
    ".ttf",
    ".eot",
    ".mp4",
    ".webm",
    ".mov",
    ".zip",
    ".tar",
    ".gz",
    ".exe",
    ".dll",
    ".so",
    ".dylib",
    ".bin",
    ".pdf",
];

#[derive(Deserialize)]
struct FindArgs {
    #[serde(default)]
    query: String,
    #[serde(default)]
    grep_keywords: Vec<String>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Clone, Debug)]
struct PassageRange {
    start_line: usize,
    end_line: usize,
    score: f64,
    snippet: String,
}

#[derive(Clone, Debug)]
struct SearchHit {
    abs_path: PathBuf,
    display_path: String,
    hash: String,
    lines: Vec<String>,
    score: f64,
    ranges: Vec<PassageRange>,
}

pub fn run(toolbox: &Toolbox, args: &Value) -> ToolResult<String> {
    let args: FindArgs = serde_json::from_value(args.clone())?;
    let mode = args
        .mode
        .as_deref()
        .unwrap_or("search")
        .trim()
        .to_ascii_lowercase();
    match mode.as_str() {
        "" | "search" => search_mode(toolbox, &args),
        "outline" => outline_mode(toolbox, &args),
        "architecture" => architecture_mode(toolbox, &args),
        "trace" => trace_mode(toolbox, &args),
        other => Err(ToolError::msg(format!(
            "unknown find mode {other:?}: use search, outline, architecture, or trace"
        ))),
    }
}

fn search_mode(toolbox: &Toolbox, args: &FindArgs) -> ToolResult<String> {
    if args.query.trim().is_empty() && args.grep_keywords.is_empty() {
        return Err(ToolError::msg("query cannot be empty"));
    }
    let base = resolve_base(toolbox, args.path.as_deref())?;
    let terms = extract_terms(&args.query, &args.grep_keywords);
    if terms.is_empty() {
        return Ok(format!(
            "no hits for {:?} (\u{03c4} {THRESHOLD:.2})",
            args.query
        ));
    }
    let files = collect_candidates(&base)?;
    let mut hits = Vec::new();
    for file in files {
        if let Some(hit) = score_file(&file, toolbox.root(), &terms) {
            if hit.score >= THRESHOLD && !hit.ranges.is_empty() {
                hits.push(hit);
            }
        }
    }
    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.display_path.cmp(&b.display_path))
    });
    record_hits_seen(toolbox, &hits);
    Ok(render_hits(&hits, args))
}

fn resolve_base(toolbox: &Toolbox, raw: Option<&str>) -> ToolResult<PathBuf> {
    match raw.map(str::trim).filter(|s| !s.is_empty()) {
        Some(p) => toolbox.jail().resolve(p),
        None => Ok(toolbox.root().to_path_buf()),
    }
}

fn collect_candidates(base: &Path) -> ToolResult<Vec<PathBuf>> {
    if base.is_file() {
        return Ok(vec![base.to_path_buf()]);
    }
    let walker = WalkBuilder::new(base)
        .require_git(false)
        .sort_by_file_path(|a, b| a.cmp(b))
        .build();
    let mut out = Vec::new();
    for entry in walker.flatten() {
        let p = entry.path();
        if p.is_file() && !should_ignore(p) {
            out.push(p.to_path_buf());
        }
    }
    Ok(out)
}

fn should_ignore(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return true;
    };
    if name.starts_with('.') {
        return true;
    }
    let lower = name.to_ascii_lowercase();
    IGNORED_EXTS.iter().any(|ext| lower.ends_with(ext))
}

fn extract_terms(query: &str, keywords: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut terms = Vec::new();
    for kw in keywords {
        let clean = trim_punct(kw).to_ascii_lowercase();
        if !clean.is_empty() && seen.insert(clean.clone()) {
            terms.push(clean);
        }
    }
    for word in query.split_whitespace() {
        let clean = trim_punct(word).to_ascii_lowercase();
        if clean.len() < 3
            || STOPWORDS.contains(&clean.as_str())
            || clean.chars().all(|c| c.is_ascii_digit())
        {
            continue;
        }
        if seen.insert(clean.clone()) {
            terms.push(clean);
        }
    }
    terms
}

fn trim_punct(s: &str) -> &str {
    s.trim_matches(|c: char| {
        matches!(
            c,
            '"' | '\''
                | '.'
                | ','
                | ';'
                | ':'
                | '!'
                | '?'
                | '('
                | ')'
                | '['
                | ']'
                | '{'
                | '}'
                | '<'
                | '>'
        )
    })
}

fn score_file(path: &Path, root: &Path, terms: &[String]) -> Option<SearchHit> {
    let bytes = fs::read(path).ok()?;
    if bytes.is_empty() || bytes.len() > 1024 * 1024 || bytes.contains(&0) {
        return None;
    }
    let text = String::from_utf8(bytes.clone()).ok()?;
    let display = path
        .strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .to_string();
    let lower_text = text.to_ascii_lowercase();
    let lower_path = display.to_ascii_lowercase();
    if !terms
        .iter()
        .any(|t| lower_text.contains(t.as_str()) || lower_path.contains(t.as_str()))
    {
        return None;
    }
    let lines: Vec<String> = lines_of(&text).into_iter().map(str::to_string).collect();
    let ranges = extract_passages(&lines, &display, terms);
    if ranges.is_empty() {
        return None;
    }
    let file_score = lexical_file_score(&display, &lower_text, &lines, terms);
    let best_passage = ranges.iter().map(|r| r.score).fold(0.0_f64, f64::max);
    let score = ((file_score + best_passage) / 2.0).min(0.95);
    Some(SearchHit {
        abs_path: path.to_path_buf(),
        display_path: display,
        hash: content_hash(&bytes),
        lines,
        score,
        ranges,
    })
}

fn extract_passages(lines: &[String], display_path: &str, terms: &[String]) -> Vec<PassageRange> {
    let mut raw = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        let lower = line.to_ascii_lowercase();
        if terms.iter().any(|t| lower.contains(t.as_str())) {
            let start = idx.saturating_sub(1) + 1;
            let end = (idx + PASSAGE_WINDOW).min(lines.len());
            let snippet = lines[start - 1..end].join("\n");
            let score = score_passage(&snippet, display_path, terms);
            raw.push(PassageRange {
                start_line: start,
                end_line: end,
                score,
                snippet,
            });
        }
    }
    let mut merged = merge_ranges(raw);
    merged.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    merged.truncate(3);
    merged.sort_by_key(|r| r.start_line);
    merged
}

fn merge_ranges(mut ranges: Vec<PassageRange>) -> Vec<PassageRange> {
    if ranges.len() <= 1 {
        return ranges;
    }
    ranges.sort_by_key(|r| r.start_line);
    let mut out = Vec::new();
    let mut cur = ranges[0].clone();
    for next in ranges.into_iter().skip(1) {
        if next.start_line <= cur.end_line + 1 {
            cur.end_line = cur.end_line.max(next.end_line);
            cur.score = cur.score.max(next.score);
        } else {
            out.push(cur);
            cur = next;
        }
    }
    out.push(cur);
    out
}

fn lexical_file_score(
    display_path: &str,
    lower_text: &str,
    lines: &[String],
    terms: &[String],
) -> f64 {
    let mut score = 0.30;
    let lower_path = display_path.to_ascii_lowercase();
    let mut matched = 0usize;
    for term in terms {
        let mut hit = false;
        if lower_path.contains(term.as_str()) {
            score += 0.15;
            hit = true;
        }
        let count = lower_text.matches(term.as_str()).count().min(5);
        if count > 0 {
            score += 0.05 * count as f64;
            hit = true;
        }
        if hit {
            matched += 1;
        }
    }
    let defs = lines
        .iter()
        .filter(|l| terms.iter().any(|t| is_definition_line(l, t)))
        .take(2)
        .count();
    score += 0.12 * defs as f64;
    score += 0.10 * (matched as f64 / terms.len().max(1) as f64);
    score.min(0.95)
}

fn score_passage(chunk: &str, display_path: &str, terms: &[String]) -> f64 {
    let mut score = 0.35;
    let lower_path = display_path.to_ascii_lowercase();
    if terms.iter().any(|t| lower_path.contains(t.as_str())) {
        score += 0.10;
    }
    if chunk
        .lines()
        .any(|l| terms.iter().any(|t| is_definition_line(l, t)))
    {
        score += 0.25;
    }
    let lower_chunk = chunk.to_ascii_lowercase();
    let hits = terms
        .iter()
        .filter(|t| lower_chunk.contains(t.as_str()))
        .count()
        .min(2);
    score += 0.10 * hits as f64;
    score.min(0.95)
}

fn is_definition_line(line: &str, term: &str) -> bool {
    let lower = line.trim_start().to_ascii_lowercase();
    if lower.starts_with("//") || lower.starts_with('#') || lower.starts_with('*') {
        return false;
    }
    const PREFIXES: &[&str] = &[
        "fn ",
        "pub fn ",
        "pub async fn ",
        "async fn ",
        "struct ",
        "pub struct ",
        "enum ",
        "pub enum ",
        "trait ",
        "pub trait ",
        "type ",
        "pub type ",
        "const ",
        "pub const ",
        "func ",
        "def ",
        "class ",
        "function ",
        "export function ",
        "export class ",
        "interface ",
        "export interface ",
    ];
    PREFIXES
        .iter()
        .any(|p| lower.starts_with(&format!("{p}{term}")))
}

fn record_hits_seen(toolbox: &Toolbox, hits: &[SearchHit]) {
    for hit in hits {
        let mut visible = Vec::new();
        for r in &hit.ranges {
            visible.extend(r.start_line as u32..=r.end_line as u32);
        }
        toolbox.seen().record_with_content(
            &hit.abs_path,
            &hit.hash,
            hit.lines.clone(),
            visible,
            false,
        );
    }
}

fn render_hits(hits: &[SearchHit], args: &FindArgs) -> String {
    if hits.is_empty() {
        return format!("no hits for {:?} (\u{03c4} {THRESHOLD:.2})", args.query);
    }
    let limit = args.limit.unwrap_or(DEFAULT_HITS).clamp(1, MAX_HITS);
    let mut out = format!(
        "{} hit(s) for {:?} (\u{03c4} {THRESHOLD:.2}), strongest first\n",
        hits.len(),
        args.query
    );
    let shown_slice = &hits[..hits.len().min(limit)];
    for (idx, hit) in shown_slice.iter().enumerate() {
        if out.len() >= MAX_BYTES {
            out.push_str(&format!(
                "\n[budget of {MAX_BYTES} bytes reached: {idx} of {} blocks shown]\n",
                shown_slice.len()
            ));
            break;
        }
        out.push_str(&format!(
            "\n{}  [grep]  {:.2}\n",
            hit.display_path, hit.score
        ));
        for r in &hit.ranges {
            let preview = preview_line(&r.snippet, &args.grep_keywords);
            out.push_str(&format!(
                "  {}:{}-{}  {:.2}  {preview}\n",
                hit.display_path, r.start_line, r.end_line, r.score
            ));
        }
    }
    if hits.len() > limit {
        let dropped = hits.len() - limit;
        let next = (limit * 2).min(MAX_HITS);
        out.push_str(&format!(
            "\n[{dropped} hit(s) past limit={limit}; pass limit={next} to see more]\n"
        ));
    }
    out.trim_end().to_string()
}

fn preview_line(snippet: &str, keywords: &[String]) -> String {
    let mut first = String::new();
    for line in snippet.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if first.is_empty() {
            first = trimmed.to_string();
        }
        if keywords.iter().any(|k| {
            trimmed
                .to_ascii_lowercase()
                .contains(&k.to_ascii_lowercase())
        }) {
            return trimmed.to_string();
        }
    }
    first
}

fn outline_mode(toolbox: &Toolbox, args: &FindArgs) -> ToolResult<String> {
    let raw = args
        .path
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| (!args.query.trim().is_empty()).then_some(args.query.as_str()))
        .ok_or_else(|| ToolError::msg("outline mode requires path or query naming a file"))?;
    let path = toolbox.jail().resolve(raw)?;
    let text = fs::read_to_string(&path)?;
    let display = path
        .strip_prefix(toolbox.root())
        .unwrap_or(&path)
        .to_string_lossy();
    let mut out = format!("[{display}] outline\n");
    for (idx, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if is_outline_decl(trimmed) {
            out.push_str(&format!("{:>6}\u{2192}{trimmed}\n", idx + 1));
        }
    }
    Ok(out)
}

fn is_outline_decl(trimmed: &str) -> bool {
    const DECLS: &[&str] = &[
        "pub fn ",
        "fn ",
        "pub async fn ",
        "async fn ",
        "pub struct ",
        "struct ",
        "pub enum ",
        "enum ",
        "pub trait ",
        "trait ",
        "impl ",
        "impl<",
        "pub type ",
        "type ",
        "pub mod ",
        "mod ",
        "func ",
        "type ",
        "def ",
        "async def ",
        "class ",
        "export function ",
        "export async function ",
        "function ",
        "export class ",
        "export interface ",
        "interface ",
        "export type ",
    ];
    DECLS.iter().any(|p| trimmed.starts_with(p))
}

fn architecture_mode(toolbox: &Toolbox, args: &FindArgs) -> ToolResult<String> {
    let base = resolve_base(toolbox, args.path.as_deref())?;
    let files = collect_candidates(&base)?;
    let mut out = format!("architecture summary ({} files):\n", files.len());
    for file in files.iter().take(100) {
        let rel = file
            .strip_prefix(toolbox.root())
            .unwrap_or(file)
            .to_string_lossy();
        out.push_str(&format!("  {rel}\n"));
    }
    Ok(out)
}

fn trace_mode(toolbox: &Toolbox, args: &FindArgs) -> ToolResult<String> {
    let symbol = args.query.trim();
    if symbol.is_empty() {
        return Err(ToolError::msg("trace mode requires query naming a symbol"));
    }
    let mut search_args = FindArgs {
        query: symbol.to_string(),
        grep_keywords: vec![symbol.to_string()],
        path: args.path.clone(),
        mode: Some("search".to_string()),
        limit: args.limit,
    };
    search_args.limit = Some(args.limit.unwrap_or(DEFAULT_HITS));
    search_mode(toolbox, &search_args)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ToolConfig;
    use serde_json::json;

    #[test]
    fn find_ranks_matching_files_and_records_seen_lines() {
        let dir = tempfile::tempdir().unwrap();
        let spill = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("widget.rs"),
            "pub fn frobulate_widget(x: i32) -> i32 {\n    x + 1\n}\n",
        )
        .unwrap();
        let tb = Toolbox::new(ToolConfig::new(dir.path(), spill.path())).unwrap();
        let res = run(
            &tb,
            &json!({
                "query": "frobulate_widget",
                "grep_keywords": ["frobulate_widget"]
            }),
        )
        .unwrap();
        assert!(res.contains("widget.rs"), "got: {res}");
        assert!(res.contains("[grep]"), "got: {res}");
    }
}
