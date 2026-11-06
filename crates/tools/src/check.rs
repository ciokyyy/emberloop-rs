//! Re-runs one named check from `.emberloop/check`: a plain command, or a JSON
//! map of names to commands whose `{args}` slot takes the `args` string.
use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::Value;

use crate::{ToolError, ToolResult, Toolbox};

const TIMEOUT_SECS: u64 = 180;
const TAIL_LINES: usize = 60;
const REFUSED_ARG_CHARS: &[char] = &[
    ';', '|', '&', '$', '`', '\n', '\r', '<', '>', '(', ')', '\\', '\'', '"', '*', '?', '~', '#',
    '!',
];

#[derive(Deserialize)]
struct CheckArgs {
    #[serde(default)]
    name: String,
    #[serde(default)]
    args: String,
}

pub async fn run(toolbox: &Toolbox, args: &Value) -> ToolResult<String> {
    let CheckArgs { name, args } = serde_json::from_value(args.clone())?;
    let raw = std::fs::read_to_string(toolbox.root().join(".emberloop").join("check"))
        .unwrap_or_default();
    let (command, display) = resolve(&raw, &name, &args)?;
    let result = crate::bash::foreground(toolbox, &command, TIMEOUT_SECS).await?;
    Ok(format_result(&display, &result))
}

/// The shell command for `name` with `args` substituted, plus the label for the status line.
fn resolve(raw: &str, name: &str, args: &str) -> ToolResult<(String, String)> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(ToolError::msg(
            "no checks configured (the repo has no .emberloop/check)",
        ));
    }
    let name = if name.is_empty() { "default" } else { name };
    let checks: Option<BTreeMap<String, String>> = raw
        .starts_with('{')
        .then(|| serde_json::from_str(raw).ok())
        .flatten()
        .filter(|checks: &BTreeMap<String, String>| !checks.is_empty());
    let Some(checks) = checks else {
        if name != "default" {
            return Err(ToolError::msg(format!(
                "unknown check {name:?}; available: default"
            )));
        }
        if !args.is_empty() {
            return Err(no_args(name));
        }
        return Ok((raw.to_string(), raw.to_string()));
    };
    let Some(command) = checks.get(name) else {
        let names: Vec<&str> = checks.keys().map(String::as_str).collect();
        return Err(ToolError::msg(format!(
            "unknown check {name:?}; available: {}",
            names.join(", ")
        )));
    };
    if command.trim().is_empty() {
        return Err(ToolError::msg(format!(
            "check {name:?} has an empty command"
        )));
    }
    let command = if command.contains("{args}") {
        if let Some(bad) = args.chars().find(|c| REFUSED_ARG_CHARS.contains(c)) {
            return Err(ToolError::msg(format!(
                "args must not contain shell metacharacters (found {bad:?})"
            )));
        }
        command.replace("{args}", args)
    } else if args.is_empty() {
        command.clone()
    } else {
        return Err(no_args(name));
    };
    let display = if name == "default" {
        command.clone()
    } else {
        format!("{name}: {command}")
    };
    Ok((command, display))
}

fn no_args(name: &str) -> ToolError {
    ToolError::msg(format!(
        "check {name:?} takes no arguments (its command has no {{args}} slot)"
    ))
}

/// `bash::foreground` output (`exit code: N` or `timeout after ...` header) as a verdict plus the output tail.
fn format_result(display: &str, result: &str) -> String {
    let (header, body) = result.split_once('\n').unwrap_or((result, ""));
    let lines: Vec<&str> = body.trim().lines().collect();
    let tail = if lines.len() > TAIL_LINES {
        format!("…\n{}", lines[lines.len() - TAIL_LINES..].join("\n"))
    } else {
        lines.join("\n")
    };
    match header.strip_prefix("exit code: ") {
        Some("0") => format!("`{display}` passed (exit 0):\n{tail}"),
        Some(code) => format!("`{display}` FAILED (exit {code}):\n{tail}"),
        None => format!("`{display}` TIMED OUT after {TIMEOUT_SECS}s, which is not a pass. Output so far:\n{tail}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_file_is_the_default_check() {
        assert_eq!(resolve("make test\n", "", "").unwrap().0, "make test");
        assert!(resolve("make test", "lint", "").is_err());
        assert!(resolve("make test", "", "x").is_err());
        assert!(resolve("  ", "", "").is_err());
    }

    #[test]
    fn map_files_name_checks_and_fill_the_args_slot() {
        let raw = r#"{"default":"cargo test","unit":"cargo test {args}"}"#;
        assert_eq!(resolve(raw, "", "").unwrap().1, "cargo test");
        let (command, display) = resolve(raw, "unit", "-p tools").unwrap();
        assert_eq!(command, "cargo test -p tools");
        assert_eq!(display, "unit: cargo test -p tools");
        assert!(resolve(raw, "unit", "x; rm -rf /").is_err());
        assert!(resolve(raw, "", "x").is_err());
        let error = resolve(raw, "nope", "").unwrap_err().to_string();
        assert!(error.contains("available: default, unit"), "{error}");
    }

    #[test]
    fn verdict_names_pass_fail_and_timeout() {
        assert!(format_result("t", "exit code: 0\nok\n").starts_with("`t` passed (exit 0):\nok"));
        assert!(format_result("t", "exit code: 2\nbad\n").starts_with("`t` FAILED (exit 2):"));
        assert!(
            format_result("t", "timeout after 180s (process group killed)\n").contains("TIMED OUT")
        );
        let long = (0..100)
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let shown = format_result("t", &format!("exit code: 0\n{long}\n"));
        assert!(shown.contains("…\n40\n") && !shown.contains("\n39\n"));
    }
}
