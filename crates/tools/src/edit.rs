use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;
use serde::Deserialize;
use serde_json::Value;

use crate::skillpath;
use crate::tag::{content_hash, content_tag, line_count, lines_of};
use crate::{ToolError, ToolResult, Toolbox};

const CONTEXT_LINES: usize = 3;
const DELETION_WARN_NET: isize = 10;

#[derive(Deserialize)]
struct EditArgs {
    input: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OpKind {
    Replace,
    InsertBefore,
    InsertAfter,
    Append,
    Delete,
}

#[derive(Clone, Debug)]
struct Op {
    kind: OpKind,
    start: usize,
    end: usize,
    body: Vec<String>,
}

struct Parsed {
    path: String,
    tag: String,
    ops: Vec<Op>,
}

fn header_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| match Regex::new(r"^\s*\[([^#\s]+)#([0-9A-Fa-f]{4})\]") {
        Ok(re) => re,
        Err(_) => unreachable!(),
    })
}

fn loose_header_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| match Regex::new(r"^\s*\[([^#\s]+)#([^\]]*)\]") {
        Ok(re) => re,
        Err(_) => unreachable!(),
    })
}

pub fn run(toolbox: &Toolbox, args: &Value) -> ToolResult<String> {
    let args: EditArgs = serde_json::from_value(args.clone())?;
    if args.input.trim().is_empty() {
        return Err(ToolError::msg("input is required"));
    }
    let mut seen_keys = HashSet::new();
    let mut prepared = Vec::new();
    for section in parse_patches(&args.input)? {
        prepared.push(prepare(toolbox, section, &mut seen_keys)?);
    }
    commit_all(toolbox, prepared)
}

struct PreparedEdit {
    path: PathBuf,
    key: PathBuf,
    label: String,
    skill: Option<skillpath::Resolved>,
    skill_op: Option<skillpath::Op>,
    original: Vec<u8>,
    actual_hash: String,
    new_text: String,
    new_hash: String,
    new_tag: String,
    applied: Applied,
    ops: Vec<Op>,
    rebase_warning: Option<String>,
}

/// Phase 1: validate one section completely, writing nothing.
fn prepare(
    toolbox: &Toolbox,
    parsed: Parsed,
    seen_keys: &mut HashSet<PathBuf>,
) -> ToolResult<PreparedEdit> {
    let skill = skillpath::writable(toolbox, &parsed.path)?;
    let (path, key, label) = match &skill {
        Some(resolved) => (
            resolved.path.clone(),
            resolved.key.clone(),
            skillpath::display(resolved),
        ),
        None => {
            let path = toolbox.jail().resolve_any(&parsed.path)?;
            (path.clone(), path.clone(), path.display().to_string())
        }
    };
    let canonical = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
    if !seen_keys.insert(canonical) {
        return Err(ToolError::msg(format!(
            "duplicate path {label} in one edit call; combine its operations under one header"
        )));
    }
    let original = fs::read(&path).map_err(|error| named(&label, ToolError::from(error)))?;
    let text = String::from_utf8(original.clone())
        .map_err(|_| ToolError::msg(format!("{label} is binary; edit only text files")))?;
    let lines = lines_of(&text);
    let actual_hash = content_hash(&original);
    let actual_tag = content_tag(&original);
    let mut ops = parsed.ops;
    let mut rebase_warning: Option<String> = None;
    if actual_tag != parsed.tag {
        let rebased_opt = toolbox
            .seen()
            .lookup_tag(&key, &parsed.tag)
            .and_then(|old_snap| {
                rebase_ops(&ops, &old_snap.content_lines, &lines, &parsed.tag)
                    .map(|(rebased, msg)| (old_snap, rebased, msg))
            });
        if let Some((old_snap, rebased, msg)) = rebased_opt {
            check_seen(
                toolbox,
                &key,
                &label,
                &old_snap.hash,
                &ops,
                old_snap.content_lines.len(),
            )?;
            ops = rebased;
            rebase_warning = Some(msg);
        } else {
            return Err(stale_tag_error(
                toolbox,
                &key,
                &label,
                &parsed.tag,
                &actual_tag,
                &original,
            ));
        }
    } else {
        check_seen(toolbox, &key, &label, &actual_hash, &ops, lines.len())?;
    }
    validate_ops(&ops, lines.len()).map_err(|error| named(&label, error))?;
    check_overlap(&ops).map_err(|error| named(&label, error))?;
    let applied = apply(&ops, &lines);
    let new_text = join_lines(&applied.lines, &text);
    let new_hash = content_hash(new_text.as_bytes());
    let new_tag = content_tag(new_text.as_bytes());
    let skill_op = match &skill {
        Some(resolved) => Some(
            skillpath::check_write(toolbox, resolved, &new_text)
                .map_err(|error| named(&label, error))?,
        ),
        None => None,
    };
    Ok(PreparedEdit {
        path,
        key,
        label,
        skill,
        skill_op,
        original,
        actual_hash,
        new_text,
        new_hash,
        new_tag,
        applied,
        ops,
        rebase_warning,
    })
}

/// Phase 2: write every entry, rolling already-written ones back on a failure.
fn commit_all(toolbox: &Toolbox, prepared: Vec<PreparedEdit>) -> ToolResult<String> {
    let mut written: Vec<usize> = Vec::new();
    for (index, entry) in prepared.iter().enumerate() {
        let outcome = ensure_unchanged(&entry.path, &entry.actual_hash).and_then(|()| {
            match (&entry.skill, entry.skill_op) {
                (Some(resolved), Some(op)) => skillpath::commit(
                    toolbox,
                    resolved,
                    op,
                    Some(&entry.original),
                    entry.new_text.as_bytes(),
                ),
                _ => fs::write(&entry.path, &entry.new_text).map_err(ToolError::from),
            }
        });
        match outcome {
            Ok(()) => written.push(index),
            Err(error) => {
                let error = named(&entry.label, error);
                return Err(rollback(&prepared, &written, error));
            }
        }
    }
    let mut replies = Vec::with_capacity(prepared.len());
    for entry in &prepared {
        toolbox.seen().record_with_content(
            &entry.key,
            &entry.new_hash,
            entry.applied.lines.clone(),
            1..=line_count(&entry.new_text) as u32,
            true,
        );
        replies.push(reply(
            &entry.label,
            &entry.new_tag,
            &entry.applied.lines,
            &entry.applied.changed,
            &entry.ops,
            entry.rebase_warning.as_deref(),
        ));
    }
    Ok(replies.join("\n\n"))
}

/// Restore entries 0..failed only where nothing else touched them since.
fn rollback(prepared: &[PreparedEdit], written: &[usize], error: ToolError) -> ToolError {
    let mut restored = 0usize;
    let mut kept = Vec::new();
    let mut unrestored = Vec::new();
    for &index in written {
        let entry = &prepared[index];
        let untouched = fs::read(&entry.path)
            .map(|current| content_hash(&current) == entry.new_hash)
            .unwrap_or(false);
        if !untouched {
            kept.push(entry.label.as_str());
        } else if fs::write(&entry.path, &entry.original).is_ok() {
            restored += 1;
        } else {
            unrestored.push(entry.label.as_str());
        }
    }
    let mut outcome = if written.is_empty() {
        "no file had been written yet".to_string()
    } else {
        format!("rolled back {restored} already-written file(s)")
    };
    if !kept.is_empty() {
        outcome.push_str(&format!(
            "; kept the concurrent external changes in {}",
            kept.join(", ")
        ));
    }
    if !unrestored.is_empty() {
        outcome.push_str(&format!("; could not restore {}", unrestored.join(", ")));
    }
    ToolError::msg(format!("{error}; {outcome}"))
}

pub(crate) fn headers(input: &str) -> Vec<&str> {
    strip_wrapper(input)
        .lines()
        .filter(|line| !line.trim_end().starts_with('+'))
        .filter_map(|line| header_regex().captures(line.trim_end()))
        .filter_map(|caps| caps.get(1).map(|m| m.as_str()))
        .collect()
}

fn strip_wrapper(input: &str) -> &str {
    input
        .trim()
        .trim_start_matches("*** Begin Patch")
        .trim_end_matches("*** End Patch")
        .trim()
}

fn parse_patches(input: &str) -> ToolResult<Vec<Parsed>> {
    let text = strip_wrapper(input);
    let mut sections: Vec<(String, String, String)> = Vec::new();
    for raw in text.lines() {
        let line = raw.trim_end();
        if let Some(caps) = header_regex().captures(line) {
            sections.push((caps[1].to_string(), caps[2].to_uppercase(), String::new()));
            continue;
        }
        if !line.starts_with('+') && loose_header_regex().is_match(line) {
            return Err(missing_header_error(line));
        }
        match sections.last_mut() {
            Some((_, _, body)) => {
                body.push_str(raw);
                body.push('\n');
            }
            None => {
                if line.trim().is_empty() || line.trim_start().starts_with("***") {
                    continue;
                }
                return Err(missing_header_error(text));
            }
        }
    }
    if sections.is_empty() {
        return Err(missing_header_error(text));
    }
    let mut parsed = Vec::with_capacity(sections.len());
    for (path, tag, body) in sections {
        let ops = parse_ops(&body).map_err(|error| named(&path, error))?;
        if ops.is_empty() {
            return Err(ToolError::msg(format!(
                "no operations in the patch for {path}, so nothing was written"
            )));
        }
        parsed.push(Parsed { path, tag, ops });
    }
    Ok(parsed)
}

fn missing_header_error(text: &str) -> ToolError {
    if let Some(loose) = loose_header_regex().captures(text) {
        return ToolError::msg(format!(
            "tag {:?} is not four hex digits; read {} and copy its [path#TAG] header",
            loose.get(2).map_or("", |m| m.as_str()),
            loose.get(1).map_or("the file", |m| m.as_str())
        ));
    }
    ToolError::msg("missing header: open the patch with [path#TAG]")
}

fn parse_ops(text: &str) -> ToolResult<Vec<Op>> {
    let put_range =
        Regex::new(r"^PUT\s+(\d+)\.=(\d+):$").map_err(|e| ToolError::msg(e.to_string()))?;
    let put_before = Regex::new(r"^PUT\s+<(\d+):$").map_err(|e| ToolError::msg(e.to_string()))?;
    let put_after = Regex::new(r"^PUT\s+>(\d+):$").map_err(|e| ToolError::msg(e.to_string()))?;
    let put_tail = Regex::new(r"^PUT\s+>\$:$").map_err(|e| ToolError::msg(e.to_string()))?;
    let cut_range =
        Regex::new(r"^CUT\s+(\d+)\.=(\d+)$").map_err(|e| ToolError::msg(e.to_string()))?;
    let cut_single = Regex::new(r"^CUT\s+(\d+)$").map_err(|e| ToolError::msg(e.to_string()))?;
    let regexes = OpRegexes {
        put_range,
        put_before,
        put_after,
        put_tail,
        cut_range,
        cut_single,
    };
    let mut ops = Vec::new();
    let mut current: Option<Op> = None;
    for raw in text.lines() {
        let line = raw.trim_end();
        if line.trim().is_empty() {
            continue;
        }
        if let Some(body) = line.strip_prefix('+') {
            let op = current
                .as_mut()
                .ok_or_else(|| ToolError::msg("body line with no operation above it"))?;
            op.body.push(body.to_string());
            continue;
        }
        if let Some(done) = current.take() {
            validate_body(&done)?;
            ops.push(done);
        }
        if line.trim_start().starts_with("***") {
            continue;
        }
        current = Some(parse_op(line.trim(), &regexes)?);
    }
    if let Some(done) = current.take() {
        validate_body(&done)?;
        ops.push(done);
    }
    Ok(ops)
}

struct OpRegexes {
    put_range: Regex,
    put_before: Regex,
    put_after: Regex,
    put_tail: Regex,
    cut_range: Regex,
    cut_single: Regex,
}

fn parse_op(line: &str, regexes: &OpRegexes) -> ToolResult<Op> {
    if let Some(c) = regexes.put_range.captures(line) {
        return Ok(op(OpKind::Replace, &c[1], &c[2]));
    }
    if let Some(c) = regexes.put_before.captures(line) {
        return Ok(op(OpKind::InsertBefore, &c[1], &c[1]));
    }
    if let Some(c) = regexes.put_after.captures(line) {
        return Ok(op(OpKind::InsertAfter, &c[1], &c[1]));
    }
    if regexes.put_tail.is_match(line) {
        return Ok(Op {
            kind: OpKind::Append,
            start: 0,
            end: 0,
            body: Vec::new(),
        });
    }
    if let Some(c) = regexes.cut_range.captures(line) {
        return Ok(op(OpKind::Delete, &c[1], &c[2]));
    }
    if let Some(c) = regexes.cut_single.captures(line) {
        return Ok(op(OpKind::Delete, &c[1], &c[1]));
    }
    Err(ToolError::msg(format!(
        "unexpected line {line:?}; use PUT N.=M:, PUT <N:, PUT >N:, PUT >$: or CUT N"
    )))
}

fn op(kind: OpKind, start: &str, end: &str) -> Op {
    Op {
        kind,
        start: start.parse().unwrap_or(0),
        end: end.parse().unwrap_or(0),
        body: Vec::new(),
    }
}

fn validate_body(op: &Op) -> ToolResult<()> {
    if op.kind == OpKind::Delete {
        if !op.body.is_empty() {
            return Err(ToolError::msg("CUT takes no body"));
        }
        return Ok(());
    }
    if op.body.is_empty() {
        return Err(ToolError::msg(
            "PUT needs at least one body line starting with '+'",
        ));
    }
    Ok(())
}

fn validate_ops(ops: &[Op], line_count: usize) -> ToolResult<()> {
    for op in ops {
        match op.kind {
            OpKind::Replace | OpKind::Delete => {
                if op.start == 0 || op.start > op.end || op.end > line_count {
                    return Err(ToolError::msg(format!(
                        "lines {}.={} are outside the file's {} lines",
                        op.start, op.end, line_count
                    )));
                }
            }
            OpKind::InsertBefore => {
                if op.start == 0 || op.start > line_count + 1 {
                    return Err(ToolError::msg(format!(
                        "PUT <{} is outside the file's {} lines",
                        op.start, line_count
                    )));
                }
            }
            OpKind::InsertAfter => {
                if op.start == 0 || op.start > line_count {
                    return Err(ToolError::msg(format!(
                        "PUT >{} is outside the file's {} lines",
                        op.start, line_count
                    )));
                }
            }
            OpKind::Append => {}
        }
    }
    Ok(())
}

fn check_overlap(ops: &[Op]) -> ToolResult<()> {
    let ranges: Vec<(usize, usize)> = ops
        .iter()
        .filter(|op| matches!(op.kind, OpKind::Replace | OpKind::Delete))
        .map(|op| (op.start, op.end))
        .collect();
    for (i, a) in ranges.iter().enumerate() {
        for b in &ranges[i + 1..] {
            if a.0 <= b.1 && b.0 <= a.1 {
                return Err(ToolError::msg(format!(
                    "operations on lines {}.={} and {}.={} overlap",
                    a.0, a.1, b.0, b.1
                )));
            }
        }
    }
    let mut insertions: Vec<(OpKind, usize)> = Vec::new();
    for op in ops {
        if !matches!(op.kind, OpKind::InsertBefore | OpKind::InsertAfter) {
            continue;
        }
        if insertions.contains(&(op.kind, op.start)) {
            return Err(ToolError::msg(format!(
                "two insertions at the same anchor line {}",
                op.start
            )));
        }
        insertions.push((op.kind, op.start));
        let inside = ranges.iter().any(|&(start, end)| match op.kind {
            OpKind::InsertBefore => op.start > start && op.start <= end,
            OpKind::InsertAfter => op.start >= start && op.start < end,
            _ => false,
        });
        if inside {
            return Err(ToolError::msg(format!(
                "insertion at line {} falls inside a replaced or deleted range",
                op.start
            )));
        }
    }
    Ok(())
}

fn op_anchors(op: &Op) -> Vec<u32> {
    match op.kind {
        OpKind::Replace | OpKind::Delete => (op.start as u32..=op.end as u32).collect(),
        OpKind::InsertBefore | OpKind::InsertAfter => vec![op.start as u32],
        OpKind::Append => Vec::new(),
    }
}

fn check_seen(
    toolbox: &Toolbox,
    key: &Path,
    label: &str,
    hash: &str,
    ops: &[Op],
    line_count: usize,
) -> ToolResult<()> {
    let tag = &hash[..4];
    let anchors: Vec<u32> = ops
        .iter()
        .flat_map(op_anchors)
        .filter(|line| *line as usize <= line_count)
        .collect();
    match toolbox.seen().unseen(key, hash, &anchors) {
        None => Err(ToolError::msg(format!(
            "tag #{tag} has no recorded view for {label}; read {label} first"
        ))),
        Some(unseen) if !unseen.is_empty() => Err(ToolError::msg(format!(
            "tag #{tag} did not show these anchor lines: {}; read {label} again with enough lines",
            unseen
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(", "),
        ))),
        Some(_) => Ok(()),
    }
}

struct Applied {
    lines: Vec<String>,
    changed: Vec<usize>,
}

fn apply(ops: &[Op], original: &[&str]) -> Applied {
    let mut before: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut after: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut starts: HashMap<usize, usize> = HashMap::new();
    let mut appends: Vec<usize> = Vec::new();
    for (index, op) in ops.iter().enumerate() {
        match op.kind {
            OpKind::InsertBefore => before.entry(op.start).or_default().push(index),
            OpKind::InsertAfter => after.entry(op.start).or_default().push(index),
            OpKind::Append => appends.push(index),
            OpKind::Replace | OpKind::Delete => {
                starts.insert(op.start, index);
            }
        }
    }
    let mut out = Vec::new();
    let mut changed = Vec::new();
    let mut line = 1;
    while line <= original.len() {
        emit_all(&before, line, ops, &mut out, &mut changed);
        if let Some(&index) = starts.get(&line) {
            let op = &ops[index];
            if op.kind == OpKind::Replace {
                emit_body(op, &mut out, &mut changed);
            } else {
                changed.push(out.len() + 1);
            }
            emit_all(&after, op.end, ops, &mut out, &mut changed);
            line = op.end + 1;
            continue;
        }
        out.push(original[line - 1].to_string());
        emit_all(&after, line, ops, &mut out, &mut changed);
        line += 1;
    }
    emit_all(&before, original.len() + 1, ops, &mut out, &mut changed);
    for &index in &appends {
        emit_body(&ops[index], &mut out, &mut changed);
    }
    Applied {
        lines: out,
        changed,
    }
}

fn emit_all(
    map: &HashMap<usize, Vec<usize>>,
    at: usize,
    ops: &[Op],
    out: &mut Vec<String>,
    changed: &mut Vec<usize>,
) {
    if let Some(indices) = map.get(&at) {
        for &index in indices {
            emit_body(&ops[index], out, changed);
        }
    }
}
fn emit_body(op: &Op, out: &mut Vec<String>, changed: &mut Vec<usize>) {
    for body_line in &op.body {
        out.push(body_line.clone());
        changed.push(out.len());
    }
}

fn join_lines(lines: &[String], original: &str) -> String {
    let mut text = lines.join("\n");
    if !lines.is_empty() && (original.is_empty() || original.ends_with('\n')) {
        text.push('\n');
    }
    text
}

/// Names the file an error came from, unless its message already does.
fn named(label: &str, error: ToolError) -> ToolError {
    let text = error.to_string();
    if text.starts_with(label) {
        error
    } else {
        ToolError::msg(format!("{label}: {text}"))
    }
}

fn ensure_unchanged(path: &Path, expected_hash: &str) -> ToolResult<()> {
    let current = fs::read(path)?;
    let now = content_hash(&current);
    if now != expected_hash {
        return Err(ToolError::msg(format!(
            "{} changed while this edit was being applied (tag {} is now {})",
            path.display(),
            &expected_hash[..4],
            &now[..4]
        )));
    }
    Ok(())
}

fn stale_tag_error(
    toolbox: &Toolbox,
    key: &Path,
    label: &str,
    expected: &str,
    actual: &str,
    original: &[u8],
) -> ToolError {
    let text = String::from_utf8_lossy(original);
    let lines = lines_of(&text);
    let shown = &lines[..lines.len().min(200)];
    // `retry_call` may only fix the tag, so these lines must not force a fresh `read`.
    toolbox
        .seen()
        .record(key, &content_hash(original), 1..=shown.len() as u32);
    let mut out = format!(
        "stale tag: {label} is #{actual}, not #{expected}. Use #{actual} in the header.\ncurrent lines:\n"
    );
    for (index, line) in shown.iter().enumerate() {
        out.push_str(&format!("{:>6}\u{2192}{line}\n", index + 1));
    }
    ToolError::msg(out)
}

fn reply(
    label: &str,
    tag: &str,
    lines: &[String],
    changed: &[usize],
    ops: &[Op],
    rebase_warning: Option<&str>,
) -> String {
    let mut out = format!(
        "[{label}#{tag}] updated successfully ({} lines)\n",
        lines.len()
    );
    if let Some(warn) = rebase_warning {
        out.push_str("Warnings: ");
        out.push_str(warn);
        out.push('\n');
    }
    let net = deletion_net(ops);
    if net >= DELETION_WARN_NET {
        out.push_str(&format!(
            "Warnings: this patch deletes {net} more lines than it adds\n"
        ));
    }
    out.push_str(&render_changed(lines, changed));
    out
}

fn rebase_ops(
    ops: &[Op],
    old_lines: &[String],
    new_lines: &[&str],
    old_tag: &str,
) -> Option<(Vec<Op>, String)> {
    let mut prefix = 0usize;
    while prefix < old_lines.len()
        && prefix < new_lines.len()
        && old_lines[prefix] == new_lines[prefix]
    {
        prefix += 1;
    }
    let mut suffix = 0usize;
    while suffix < old_lines.len().saturating_sub(prefix)
        && suffix < new_lines.len().saturating_sub(prefix)
        && old_lines[old_lines.len() - 1 - suffix] == new_lines[new_lines.len() - 1 - suffix]
    {
        suffix += 1;
    }
    let middle_end = old_lines.len() - suffix;
    let shift = new_lines.len() as isize - old_lines.len() as isize;
    let mut rebased = Vec::with_capacity(ops.len());
    let mut active_delta: Option<isize> = None;
    for op in ops {
        for anchor in op_anchors(op) {
            let n = anchor as usize;
            if n >= prefix && n <= middle_end + 1 {
                return None;
            }
        }
        let mut next = op.clone();
        if op.kind != OpKind::Append {
            let delta = if op.start > prefix { shift } else { 0 };
            if let Some(prev) = active_delta {
                if prev != delta {
                    return None;
                }
            }
            active_delta = Some(delta);
            next.start = (op.start as isize + delta) as usize;
            next.end = (op.end as isize + delta) as usize;
        }
        rebased.push(next);
    }
    let effective = active_delta.unwrap_or(0);
    let msg = format!(
        "tag {old_tag} was stale; rebased {} ops, lines shifted by {effective:+}",
        ops.len()
    );
    Some((rebased, msg))
}

fn render_changed(lines: &[String], changed: &[usize]) -> String {
    if changed.is_empty() || lines.is_empty() {
        return String::new();
    }
    let mut points = changed.to_vec();
    points.sort_unstable();
    points.dedup();
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for point in points {
        let start = point.saturating_sub(CONTEXT_LINES).max(1);
        let end = (point + CONTEXT_LINES).min(lines.len());
        match ranges.last_mut() {
            Some(last) if start <= last.1 + 1 => last.1 = last.1.max(end),
            _ => ranges.push((start, end)),
        }
    }
    let mut out = String::new();
    for (index, (start, end)) in ranges.into_iter().enumerate() {
        if index > 0 {
            out.push_str("--\n");
        }
        for line_no in start..=end {
            out.push_str(&format!("{:>6}\u{2192}{}\n", line_no, lines[line_no - 1]));
        }
    }
    out
}

fn deletion_net(ops: &[Op]) -> isize {
    let mut deleted = 0isize;
    let mut added = 0isize;
    for op in ops {
        match op.kind {
            OpKind::Replace => {
                deleted += (op.end - op.start + 1) as isize;
                added += op.body.len() as isize;
            }
            OpKind::Delete => deleted += (op.end - op.start + 1) as isize,
            OpKind::InsertBefore | OpKind::InsertAfter | OpKind::Append => {
                added += op.body.len() as isize;
            }
        }
    }
    deleted - added
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ToolConfig;
    use serde_json::json;

    fn toolbox(dir: &Path) -> Toolbox {
        let root = dir.join("project");
        fs::create_dir_all(&root).unwrap();
        Toolbox::new(ToolConfig::new(&root, dir.join("spill"))).unwrap()
    }

    fn tag_of(toolbox: &Toolbox, path: &str) -> String {
        let view = crate::read::run(toolbox, &json!({ "path": path })).unwrap();
        let header = view.lines().next().unwrap();
        header.rsplit_once('#').unwrap().1[..4].to_string()
    }

    #[test]
    fn multi_file_patch_writes_every_file_and_joins_replies() {
        let dir = tempfile::tempdir().unwrap();
        let toolbox = toolbox(dir.path());
        let project = dir.path().join("project");
        fs::write(project.join("a.txt"), "one\n").unwrap();
        fs::write(project.join("b.txt"), "two\n").unwrap();
        let a_tag = tag_of(&toolbox, "a.txt");
        let b_tag = tag_of(&toolbox, "b.txt");

        let reply = run(
            &toolbox,
            &json!({ "input": format!(
                "[a.txt#{a_tag}]\nPUT 1.=1:\n+one changed\n\n[b.txt#{b_tag}]\nPUT 1.=1:\n+two changed\n"
            )}),
        )
        .unwrap();

        assert_eq!(
            fs::read_to_string(project.join("a.txt")).unwrap(),
            "one changed\n"
        );
        assert_eq!(
            fs::read_to_string(project.join("b.txt")).unwrap(),
            "two changed\n"
        );
        for name in ["a.txt", "b.txt"] {
            let path = project.join(name);
            let hash = content_hash(&fs::read(&path).unwrap());
            assert_eq!(
                toolbox.seen().unseen(&path, &hash, &[1]),
                Some(Vec::new()),
                "{name} must authorize its own lines after the batch"
            );
        }
        let parts: Vec<&str> = reply.split("\n\n").collect();
        assert_eq!(parts.len(), 2, "{reply}");
        assert!(parts[0].contains("a.txt#"), "{reply}");
        assert!(parts[1].contains("b.txt#"), "{reply}");
        let tag = parts[0].split('#').nth(1).unwrap();
        assert_eq!(tag_of(&toolbox, "a.txt"), tag[..4].to_string());
    }

    #[test]
    fn a_failing_validation_in_a_later_file_leaves_earlier_files_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let toolbox = toolbox(dir.path());
        let project = dir.path().join("project");
        fs::write(project.join("a.txt"), "one\n").unwrap();
        fs::write(project.join("b.txt"), "two\n").unwrap();
        let a_tag = tag_of(&toolbox, "a.txt");
        tag_of(&toolbox, "b.txt");

        let error = run(
            &toolbox,
            &json!({ "input": format!(
                "[a.txt#{a_tag}]\nPUT 1.=1:\n+one changed\n\n[b.txt#0000]\nPUT 1.=1:\n+two changed\n"
            )}),
        )
        .unwrap_err();

        assert!(error.to_string().contains("b.txt"), "{error}");
        assert_eq!(fs::read_to_string(project.join("a.txt")).unwrap(), "one\n");
        assert_eq!(fs::read_to_string(project.join("b.txt")).unwrap(), "two\n");
    }

    #[test]
    fn duplicate_canonical_paths_are_rejected_before_any_write() {
        let dir = tempfile::tempdir().unwrap();
        let toolbox = toolbox(dir.path());
        let project = dir.path().join("project");
        fs::write(project.join("a.txt"), "one\n").unwrap();
        let tag = tag_of(&toolbox, "a.txt");

        let error = run(
            &toolbox,
            &json!({ "input": format!(
                "[a.txt#{tag}]\nPUT 1.=1:\n+one changed\n\n[./a.txt#{tag}]\nPUT 1.=1:\n+other\n"
            )}),
        )
        .unwrap_err();

        assert!(
            error.to_string().contains("duplicate path")
                && error
                    .to_string()
                    .contains("combine its operations under one header"),
            "{error}"
        );
        assert_eq!(fs::read_to_string(project.join("a.txt")).unwrap(), "one\n");
    }

    #[test]
    fn a_malformed_later_header_errors_naming_that_file() {
        let dir = tempfile::tempdir().unwrap();
        let toolbox = toolbox(dir.path());
        let project = dir.path().join("project");
        fs::write(project.join("a.txt"), "one\n").unwrap();
        fs::write(project.join("b.txt"), "two\n").unwrap();
        let a_tag = tag_of(&toolbox, "a.txt");

        let error = run(
            &toolbox,
            &json!({ "input": format!(
                "[a.txt#{a_tag}]\nPUT 1.=1:\n+one changed\n\n[b.txt#nope]\nPUT 1.=1:\n+two changed\n"
            )}),
        )
        .unwrap_err();

        assert!(error.to_string().contains("b.txt"), "{error}");
        assert_eq!(fs::read_to_string(project.join("a.txt")).unwrap(), "one\n");
        assert_eq!(fs::read_to_string(project.join("b.txt")).unwrap(), "two\n");
    }

    #[test]
    fn a_plus_prefixed_header_looking_line_is_written_as_file_text() {
        let dir = tempfile::tempdir().unwrap();
        let toolbox = toolbox(dir.path());
        let project = dir.path().join("project");
        fs::write(project.join("a.txt"), "one\n").unwrap();
        let tag = tag_of(&toolbox, "a.txt");

        run(
            &toolbox,
            &json!({ "input": format!(
                "[a.txt#{tag}]\nPUT 1.=1:\n+[foo#1A2B]\n"
            )}),
        )
        .unwrap();

        assert_eq!(
            fs::read_to_string(project.join("a.txt")).unwrap(),
            "[foo#1A2B]\n"
        );
    }

    fn prepare_input(toolbox: &Toolbox, input: &str) -> Vec<PreparedEdit> {
        let mut seen_keys = HashSet::new();
        parse_patches(input)
            .unwrap()
            .into_iter()
            .map(|section| prepare(toolbox, section, &mut seen_keys).unwrap())
            .collect()
    }

    #[test]
    fn an_invalid_op_in_a_later_file_names_that_file() {
        let dir = tempfile::tempdir().unwrap();
        let toolbox = toolbox(dir.path());
        let project = dir.path().join("project");
        fs::write(project.join("a.txt"), "one\n").unwrap();
        fs::write(project.join("b.txt"), "two\n").unwrap();
        let a_tag = tag_of(&toolbox, "a.txt");
        let b_tag = tag_of(&toolbox, "b.txt");

        let error = run(
            &toolbox,
            &json!({ "input": format!(
                "[a.txt#{a_tag}]\nPUT 1.=1:\n+one changed\n\n[b.txt#{b_tag}]\nPUT 9.=9:\n+two changed\n"
            )}),
        )
        .unwrap_err();

        assert!(error.to_string().contains("b.txt"), "{error}");
        assert_eq!(fs::read_to_string(project.join("a.txt")).unwrap(), "one\n");
    }

    #[test]
    fn a_phase_two_concurrent_change_rolls_back_the_earlier_file() {
        let dir = tempfile::tempdir().unwrap();
        let toolbox = toolbox(dir.path());
        let project = dir.path().join("project");
        fs::write(project.join("a.txt"), "one\n").unwrap();
        fs::write(project.join("b.txt"), "two\n").unwrap();
        let a_tag = tag_of(&toolbox, "a.txt");
        let b_tag = tag_of(&toolbox, "b.txt");
        let prepared = prepare_input(
            &toolbox,
            &format!(
                "[a.txt#{a_tag}]\nPUT 1.=1:\n+one changed\n\n[b.txt#{b_tag}]\nPUT 1.=1:\n+two changed\n"
            ),
        );

        fs::write(project.join("b.txt"), "two external\n").unwrap();
        let error = commit_all(&toolbox, prepared).unwrap_err();

        let message = error.to_string();
        assert!(message.contains("b.txt"), "{message}");
        assert!(message.contains("rolled back 1"), "{message}");
        assert_eq!(fs::read_to_string(project.join("a.txt")).unwrap(), "one\n");
        assert_eq!(
            fs::read_to_string(project.join("b.txt")).unwrap(),
            "two external\n"
        );
        let hash = content_hash(&fs::read(project.join("a.txt")).unwrap());
        assert_eq!(
            toolbox.seen().unseen(&project.join("a.txt"), &hash, &[1]),
            Some(Vec::new()),
            "a rollback must keep the pre-edit seen snapshot"
        );
    }

    #[test]
    fn rollback_keeps_a_concurrent_external_write() {
        let dir = tempfile::tempdir().unwrap();
        let toolbox = toolbox(dir.path());
        let project = dir.path().join("project");
        fs::write(project.join("a.txt"), "one\n").unwrap();
        fs::write(project.join("b.txt"), "two\n").unwrap();
        let a_tag = tag_of(&toolbox, "a.txt");
        let b_tag = tag_of(&toolbox, "b.txt");
        let prepared = prepare_input(
            &toolbox,
            &format!(
                "[a.txt#{a_tag}]\nPUT 1.=1:\n+one changed\n\n[b.txt#{b_tag}]\nPUT 1.=1:\n+two changed\n"
            ),
        );

        fs::write(project.join("a.txt"), "one external\n").unwrap();
        let error = rollback(&prepared, &[0], ToolError::msg("b.txt write failed"));

        assert!(
            error
                .to_string()
                .contains("kept the concurrent external changes"),
            "{error}"
        );
        assert_eq!(
            fs::read_to_string(project.join("a.txt")).unwrap(),
            "one external\n"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_phase_two_write_failure_rolls_back_the_committed_files() {
        let dir = tempfile::tempdir().unwrap();
        let toolbox = toolbox(dir.path());
        let project = dir.path().join("project");
        fs::write(project.join("a.txt"), "one\n").unwrap();
        let a_tag = tag_of(&toolbox, "a.txt");
        let readonly = "/proc/sys/kernel/ostype";
        let readonly_tag = tag_of(&toolbox, readonly);

        let error = run(
            &toolbox,
            &json!({ "input": format!(
                "[a.txt#{a_tag}]\nPUT 1.=1:\n+one changed\n\n[{readonly}#{readonly_tag}]\nPUT 1.=1:\n+Linux\n"
            )}),
        )
        .unwrap_err();

        let message = error.to_string();
        assert!(message.contains(readonly), "{message}");
        assert!(message.contains("rolled back"), "{message}");
        assert_eq!(fs::read_to_string(project.join("a.txt")).unwrap(), "one\n");
        let hash = content_hash(&fs::read(project.join("a.txt")).unwrap());
        assert_eq!(
            toolbox.seen().unseen(&project.join("a.txt"), &hash, &[1]),
            Some(Vec::new()),
            "rollback must keep the pre-edit seen snapshot"
        );
    }
}
