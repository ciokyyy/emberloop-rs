use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::OnceLock;

use regex::Regex;
use serde::Deserialize;
use serde_json::Value;

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
    RE.get_or_init(
        || match Regex::new(r"^\s*\[([^#\s\]]+)#([0-9A-Fa-f]{4})\]") {
            Ok(re) => re,
            Err(_) => unreachable!(),
        },
    )
}

fn loose_header_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| match Regex::new(r"^\s*\[([^\]\s]+)#([^\]]*)\]") {
        Ok(re) => re,
        Err(_) => unreachable!(),
    })
}

pub fn run(toolbox: &Toolbox, args: &Value) -> ToolResult<String> {
    let args: EditArgs = serde_json::from_value(args.clone())?;
    if args.input.trim().is_empty() {
        return Err(ToolError::msg("input is required"));
    }
    let mut parsed = parse_patch(&args.input)?;
    let path = toolbox.jail().resolve(&parsed.path)?;
    let original = fs::read(&path)?;
    let text = String::from_utf8(original.clone()).map_err(|_| {
        ToolError::msg(format!(
            "{} is binary; edit only text files",
            path.display()
        ))
    })?;
    let lines = lines_of(&text);
    let actual_hash = content_hash(&original);
    let actual_tag = content_tag(&original);
    let mut rebase_warning: Option<String> = None;
    if actual_tag != parsed.tag {
        let rebased_opt = toolbox
            .seen()
            .lookup_tag(&path, &parsed.tag)
            .and_then(|old_snap| {
                rebase_ops(&parsed.ops, &old_snap.content_lines, &lines, &parsed.tag)
                    .map(|(rebased, msg)| (old_snap, rebased, msg))
            });
        if let Some((old_snap, rebased, msg)) = rebased_opt {
            check_seen(
                toolbox,
                &path,
                &old_snap.hash,
                &parsed.ops,
                old_snap.content_lines.len(),
            )?;
            parsed.ops = rebased;
            rebase_warning = Some(msg);
        } else {
            return Err(stale_tag_error(&path, &parsed.tag, &actual_tag, &original));
        }
    } else {
        check_seen(toolbox, &path, &actual_hash, &parsed.ops, lines.len())?;
    }
    validate_ops(&parsed.ops, lines.len())?;
    check_overlap(&parsed.ops)?;
    let applied = apply(&parsed.ops, &lines);
    let new_text = join_lines(&applied.lines, &text);
    let new_hash = content_hash(new_text.as_bytes());
    let new_tag = content_tag(new_text.as_bytes());
    ensure_unchanged(&path, &actual_hash)?;
    fs::write(&path, &new_text)?;
    toolbox.seen().record_with_content(
        &path,
        &new_hash,
        applied.lines.clone(),
        1..=line_count(&new_text) as u32,
        true,
    );
    Ok(reply(
        &path,
        &new_tag,
        &applied.lines,
        &applied.changed,
        &parsed.ops,
        rebase_warning.as_deref(),
    ))
}

fn parse_patch(input: &str) -> ToolResult<Parsed> {
    let text = input
        .trim()
        .trim_start_matches("*** Begin Patch")
        .trim_end_matches("*** End Patch")
        .trim();
    let header = header_regex()
        .captures(text)
        .ok_or_else(|| missing_header_error(text))?;
    let path = match header.get(1) {
        Some(m) => m.as_str().to_string(),
        None => return Err(missing_header_error(text)),
    };
    let tag = match header.get(2) {
        Some(m) => m.as_str().to_uppercase(),
        None => return Err(missing_header_error(text)),
    };
    let ops = parse_ops(text)?;
    if ops.is_empty() {
        return Err(ToolError::msg(
            "no operations in the patch, so nothing was written",
        ));
    }
    Ok(Parsed { path, tag, ops })
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
        if header_regex().is_match(line) || line.trim_start().starts_with("***") {
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
    path: &Path,
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
    match toolbox.seen().unseen(path, hash, &anchors) {
        None => Err(ToolError::msg(format!(
            "tag #{tag} has no recorded view for {}; read {} first",
            path.display(),
            path.display()
        ))),
        Some(unseen) if !unseen.is_empty() => Err(ToolError::msg(format!(
            "tag #{tag} did not show these anchor lines: {}; read {} again with enough lines",
            unseen
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(", "),
            path.display()
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

fn ensure_unchanged(path: &Path, expected_hash: &str) -> ToolResult<()> {
    let current = fs::read(path)?;
    let now = content_hash(&current);
    if now != expected_hash {
        return Err(ToolError::msg(format!(
            "{} changed while this edit was being applied (tag {} is now {}); nothing was written",
            path.display(),
            &expected_hash[..4],
            &now[..4]
        )));
    }
    Ok(())
}

fn stale_tag_error(path: &Path, expected: &str, actual: &str, original: &[u8]) -> ToolError {
    let text = String::from_utf8_lossy(original);
    let lines = lines_of(&text);
    let shown = &lines[..lines.len().min(200)];
    let mut out = format!(
        "stale tag: {} is #{actual}, not #{expected}. Read {} and copy the current tag.\ncurrent lines:\n",
        path.display(),
        path.display()
    );
    for (index, line) in shown.iter().enumerate() {
        out.push_str(&format!("{:>6}\u{2192}{line}\n", index + 1));
    }
    ToolError::msg(out)
}

fn reply(
    path: &Path,
    tag: &str,
    lines: &[String],
    changed: &[usize],
    ops: &[Op],
    rebase_warning: Option<&str>,
) -> String {
    let mut out = format!(
        "[{}#{tag}] updated successfully ({} lines)\n",
        path.display(),
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
