use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

const MAX_DIFF_BYTES: u64 = 200 * 1024;
const MAX_DIFF_LINES: usize = 2000;
const MAX_SNAPSHOTS: usize = 200;

#[derive(Default)]
pub struct ToolTracker {
    entries: HashMap<String, TrackedTool>,
    spill_dir: Option<PathBuf>,
}

struct TrackedTool {
    name: String,
    input: Value,
    snapshot: Option<(String, String)>,
}

impl ToolTracker {
    pub fn with_spill_dir(spill_dir: impl Into<PathBuf>) -> Self {
        Self {
            entries: HashMap::new(),
            spill_dir: Some(spill_dir.into()),
        }
    }

    pub fn on_start(
        &mut self,
        id: &str,
        name: &str,
        input: &Value,
        workdir: &Path,
    ) -> Option<Value> {
        if self.entries.len() >= MAX_SNAPSHOTS {
            if let Some(first_key) = self.entries.keys().next().cloned() {
                self.entries.remove(&first_key);
            }
        }
        let snapshot = if is_write_tool(name) {
            extract_target_path(input, workdir)
                .and_then(|(label, abs)| read_snapshot(&abs).map(|before| (label, before)))
        } else {
            None
        };
        let display = if is_bash_tool(name) {
            Some(classify_bash_display(input))
        } else {
            None
        };
        if !name.is_empty()
            || !input.as_object().is_some_and(|o| o.is_empty())
            || snapshot.is_some()
        {
            let mut entry = self.entries.remove(id).unwrap_or_else(|| TrackedTool {
                name: String::new(),
                input: json!({}),
                snapshot: None,
            });
            if !name.is_empty() {
                entry.name = name.to_string();
            }
            if !input.as_object().is_some_and(|o| o.is_empty()) {
                entry.input = input.clone();
            }
            if snapshot.is_some() {
                entry.snapshot = snapshot;
            }
            self.entries.insert(id.to_string(), entry);
        }
        display
    }

    pub fn on_finish(
        &mut self,
        id: &str,
        workdir: &Path,
    ) -> (Option<String>, Option<Value>, Option<Value>, Option<String>) {
        let Some(entry) = self.entries.remove(id) else {
            return (None, None, None, None);
        };
        let captured_output = self
            .spill_dir
            .as_deref()
            .and_then(|dir| consume_captured_output(dir, &entry.name, &entry.input));
        let mut display = None;
        if let Some((label, before)) = entry.snapshot {
            let abs = resolve_path(&label, workdir);
            if let Some(after) = read_existing_file(&abs) {
                let diff = unified_diff(&before, &after, &label);
                if !diff.is_empty() {
                    display = Some(json!({ "diff": diff }));
                }
            }
        }
        let name = (!entry.name.is_empty()).then_some(entry.name);
        let input = (!entry.input.as_object().is_some_and(|o| o.is_empty())).then_some(entry.input);
        (name, input, display, captured_output)
    }
}

#[derive(serde::Deserialize)]
struct CapturedOutput {
    name: String,
    args: Value,
    output: String,
}

fn normalize_mcp_args(input: &Value) -> Value {
    let inner = input
        .get("arguments")
        .or_else(|| input.get("Arguments"))
        .unwrap_or(input);
    let Some(obj) = inner.as_object() else {
        return inner.clone();
    };
    let mut clean = obj.clone();
    clean.remove("toolAction");
    clean.remove("toolSummary");
    Value::Object(clean)
}

fn consume_captured_output(spill_dir: &Path, name: &str, input: &Value) -> Option<String> {
    let dir = spill_dir.join("mcp_outputs");
    let Ok(items) = std::fs::read_dir(&dir) else {
        return None;
    };
    let mut paths: Vec<PathBuf> = items
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("json"))
        .collect();
    paths.sort();
    let target_name = short_tool_name(name);
    let target_args = normalize_mcp_args(input);
    for path in paths {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(record) = serde_json::from_str::<CapturedOutput>(&text) else {
            continue;
        };
        if short_tool_name(&record.name) == target_name
            && normalize_mcp_args(&record.args) == target_args
        {
            let _ = std::fs::remove_file(&path);
            return Some(record.output);
        }
    }
    None
}

pub(crate) fn short_tool_name(name: &str) -> &str {
    let base = name.rsplit(['/', ':']).next().unwrap_or(name);
    let stripped = base
        .strip_prefix("mcp_el_")
        .or_else(|| base.strip_prefix("mcp__el__"))
        .or_else(|| base.strip_prefix("el_"))
        .unwrap_or(base);
    stripped.trim()
}

fn is_write_tool(name: &str) -> bool {
    matches!(
        short_tool_name(name).to_ascii_lowercase().as_str(),
        "edit" | "write" | "multiedit" | "str_replace"
    )
}

fn is_bash_tool(name: &str) -> bool {
    short_tool_name(name).eq_ignore_ascii_case("bash")
}

fn classify_bash_display(input: &Value) -> Value {
    let background = input
        .get("background")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if background {
        return json!({ "v": "1", "kind": "run" });
    }
    let cmd = input
        .get("command")
        .or_else(|| input.get("cmd"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let (kind, parts) = classify_command(cmd);
    if parts.is_empty() {
        json!({ "v": "1", "kind": kind })
    } else {
        let parts_str = serde_json::to_string(&parts).unwrap_or_default();
        json!({ "v": "1", "kind": kind, "parts": parts_str })
    }
}

fn classify_command(cmd: &str) -> (&'static str, Vec<Value>) {
    if cmd.is_empty() || cmd.contains(['>', '|', ';', '&', '`']) || cmd.contains("$(") {
        return ("run", Vec::new());
    }
    let words: Vec<&str> = cmd.split_whitespace().collect();
    let Some(&first) = words.first() else {
        return ("run", Vec::new());
    };
    let prog = first.rsplit('/').next().unwrap_or(first);
    let target = words
        .iter()
        .skip(1)
        .rev()
        .find(|w| !w.starts_with('-'))
        .copied()
        .unwrap_or(".");
    match prog {
        "cat" | "head" | "tail" | "sed" | "nl" => {
            ("read", vec![json!({ "kind": "read", "target": target })])
        }
        "rg" | "grep" => (
            "search",
            vec![json!({ "kind": "search", "target": target })],
        ),
        "ls" | "find" | "tree" => ("list", vec![json!({ "kind": "list", "target": target })]),
        _ => ("run", Vec::new()),
    }
}

fn extract_target_path(input: &Value, workdir: &Path) -> Option<(String, PathBuf)> {
    for key in ["path", "filePath", "file_path", "file"] {
        if let Some(p) = input.get(key).and_then(Value::as_str) {
            let trimmed = p.trim();
            if !trimmed.is_empty() {
                return Some((trimmed.to_string(), resolve_path(trimmed, workdir)));
            }
        }
    }
    for key in ["input", "patch", "text", "content"] {
        if let Some(text) = input.get(key).and_then(Value::as_str) {
            if let Some(p) = patch_header_path(text) {
                return Some((p.clone(), resolve_path(&p, workdir)));
            }
        }
    }
    None
}

pub fn patch_header_path(patch: &str) -> Option<String> {
    for line in patch.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let inner = line.strip_prefix('[')?.strip_suffix(']')?;
        let path = match inner.rfind('#') {
            Some(idx) if idx > 0 => inner[..idx].trim(),
            _ => inner.trim(),
        };
        return (!path.is_empty()).then(|| path.to_string());
    }
    None
}

fn resolve_path(path: &str, workdir: &Path) -> PathBuf {
    let p = Path::new(path);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        workdir.join(p)
    }
}

fn read_snapshot(path: &Path) -> Option<String> {
    match std::fs::metadata(path) {
        Ok(meta) => {
            if meta.is_dir() || meta.len() > MAX_DIFF_BYTES {
                return None;
            }
            read_existing_file(path)
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Some(String::new()),
        Err(_) => None,
    }
}

fn read_existing_file(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.is_dir() || meta.len() > MAX_DIFF_BYTES {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    if text.lines().count() > MAX_DIFF_LINES {
        return None;
    }
    Some(text)
}

pub fn unified_diff(before: &str, after: &str, label: &str) -> String {
    if before == after {
        return String::new();
    }
    let old_lines = split_lines(before);
    let new_lines = split_lines(after);
    let ops = diff_ops(&old_lines, &new_lines);
    if ops.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    let label = if label.trim().is_empty() {
        "file"
    } else {
        label.trim()
    };
    if before.trim().is_empty() {
        let _ = writeln!(out, "--- /dev/null\n+++ b/{label}");
    } else {
        let _ = writeln!(out, "--- a/{label}\n+++ b/{label}");
    }
    format_hunks(&ops, &mut out);
    out
}

fn format_hunks(ops: &[(char, &str)], out: &mut String) {
    const CTX: usize = 3;
    let mut i = 0;
    while i < ops.len() {
        while i < ops.len() && ops[i].0 == ' ' {
            i += 1;
        }
        if i >= ops.len() {
            break;
        }
        let start = i.saturating_sub(CTX);
        let mut last = i;
        for (j, op) in ops.iter().enumerate().skip(i) {
            if op.0 != ' ' {
                last = j;
            }
            if j.saturating_sub(last) > CTX * 2 {
                break;
            }
        }
        let end = (last + CTX + 1).min(ops.len());
        write_hunk(&ops[..start], &ops[start..end], out);
        i = end;
    }
}

fn write_hunk(prefix: &[(char, &str)], slice: &[(char, &str)], out: &mut String) {
    let (mut old_start, mut new_start) = (1usize, 1usize);
    for (kind, _) in prefix {
        if *kind != '+' {
            old_start += 1;
        }
        if *kind != '-' {
            new_start += 1;
        }
    }
    let old_count = slice.iter().filter(|(k, _)| *k != '+').count();
    let new_count = slice.iter().filter(|(k, _)| *k != '-').count();
    let _ = writeln!(
        out,
        "@@ -{old_start},{old_count} +{new_start},{new_count} @@"
    );
    for (kind, text) in slice {
        let _ = writeln!(out, "{kind}{text}");
    }
}

fn split_lines(text: &str) -> Vec<&str> {
    if text.is_empty() {
        return Vec::new();
    }
    text.strip_suffix('\n')
        .unwrap_or(text)
        .split('\n')
        .collect()
}

fn diff_ops<'a>(a: &[&'a str], b: &[&'a str]) -> Vec<(char, &'a str)> {
    if (a.is_empty() && b.is_empty()) || a.len() * b.len() > 4_000_000 {
        return Vec::new();
    }
    let lcs = build_lcs_table(a, b);
    let mut ops = Vec::with_capacity(a.len() + b.len());
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        if a[i] == b[j] {
            ops.push((' ', a[i]));
            i += 1;
            j += 1;
        } else if lcs[i + 1][j] >= lcs[i][j + 1] {
            ops.push(('-', a[i]));
            i += 1;
        } else {
            ops.push(('+', b[j]));
            j += 1;
        }
    }
    for item in &a[i..] {
        ops.push(('-', item));
    }
    for item in &b[j..] {
        ops.push(('+', item));
    }
    ops
}

fn build_lcs_table(a: &[&str], b: &[&str]) -> Vec<Vec<usize>> {
    let mut lcs = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            lcs[i][j] = if a[i] == b[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    lcs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_patch_header_path_and_generates_unified_diff() {
        assert_eq!(
            patch_header_path("[src/main.rs#ABCD]\nPUT 1.=1:\n+fn main() {}"),
            Some("src/main.rs".into())
        );
        let diff = unified_diff(
            "line 1\nold\nline 3\n",
            "line 1\nnew\nline 3\n",
            "src/main.rs",
        );
        assert!(diff.contains("--- a/src/main.rs"));
        assert!(diff.contains("+++ b/src/main.rs"));
        assert!(diff.contains("-old"));
        assert!(diff.contains("+new"));
    }

    #[test]
    fn consumes_captured_mcp_outputs_in_fifo_order_by_tool_and_args() {
        let dir = tempfile::tempdir().unwrap();
        let out_dir = dir.path().join("mcp_outputs");
        std::fs::create_dir_all(&out_dir).unwrap();
        std::fs::write(
            out_dir.join("0001.json"),
            r#"{"name":"read","args":{"path":"a.rs"},"output":"first-a"}"#,
        )
        .unwrap();
        std::fs::write(
            out_dir.join("0002.json"),
            r#"{"name":"read","args":{"path":"b.rs"},"output":"first-b"}"#,
        )
        .unwrap();
        std::fs::write(
            out_dir.join("0003.json"),
            r#"{"name":"read","args":{"path":"a.rs"},"output":"second-a"}"#,
        )
        .unwrap();

        let mut tracker = ToolTracker::with_spill_dir(dir.path());
        tracker.on_start(
            "c1",
            "el_read",
            &json!({"arguments": {"path": "a.rs"}, "path": "a.rs"}),
            dir.path(),
        );
        tracker.on_start(
            "c2",
            "el_read",
            &json!({"arguments": {"path": "b.rs"}}),
            dir.path(),
        );
        tracker.on_start(
            "c3",
            "el_read",
            &json!({"arguments": {"path": "a.rs"}}),
            dir.path(),
        );
        tracker.on_start(
            "c4",
            "el_read",
            &json!({"arguments": {"path": "missing.rs"}}),
            dir.path(),
        );

        // Out-of-order finish still resolves each call's own output and FIFO for identical calls.
        let (_, _, _, out_b) = tracker.on_finish("c2", dir.path());
        assert_eq!(out_b.as_deref(), Some("first-b"));
        let (_, _, _, out_a1) = tracker.on_finish("c1", dir.path());
        assert_eq!(out_a1.as_deref(), Some("first-a"));
        let (_, _, _, out_a2) = tracker.on_finish("c3", dir.path());
        assert_eq!(out_a2.as_deref(), Some("second-a"));
        let (_, _, _, out_missing) = tracker.on_finish("c4", dir.path());
        assert_eq!(out_missing, None);
        assert_eq!(std::fs::read_dir(&out_dir).unwrap().count(), 0);
    }
}
