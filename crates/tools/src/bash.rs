use std::fs;
use std::process::Stdio;
use std::time::Duration;

use regex::Regex;
use serde::Deserialize;
use serde_json::Value;
use tokio::io::AsyncReadExt;

use crate::intercept;
use crate::tag::{content_hash, line_count, lines_of};
use crate::{ToolError, ToolResult, Toolbox};

const DEFAULT_TIMEOUT_SECS: u64 = 120;
const MAX_TIMEOUT_SECS: u64 = 600;
const VIEW_LINES: usize = 10;
const REWRITE_TIMEOUT: Duration = Duration::from_millis(500);

#[derive(Deserialize)]
struct BashArgs {
    command: String,
    #[serde(default)]
    timeout: Option<u64>,
    #[serde(default)]
    background: bool,
}

pub async fn run(toolbox: &Toolbox, args: &Value) -> ToolResult<String> {
    let args: BashArgs = serde_json::from_value(args.clone())?;
    if args.command.trim().is_empty() {
        return Err(ToolError::msg("command is required"));
    }
    if toolbox.is_scout() {
        if let Some(reason) = intercept::scout_blocked(&args.command) {
            return Err(ToolError::msg(format!(
                "Blocked: {reason}\nCommand not run: {}",
                args.command.trim()
            )));
        }
    }
    // A command that belongs to another tool is refused before anything runs, and the reply names
    // the tool that does it. Reads never match, so their views still feed the seen-line store.
    if let Some(rule) = intercept::blocked(&args.command) {
        return Err(ToolError::msg(format!(
            "Blocked: {}\nCommand not run: {}\nUse the {} tool instead.",
            rule.message,
            args.command.trim(),
            rule.tool
        )));
    }
    if args.background {
        return toolbox
            .background()
            .spawn(&args.command, &toolbox.env_prelude())
            .await;
    }
    let timeout = toolbox.call_secs(args.timeout, DEFAULT_TIMEOUT_SECS, MAX_TIMEOUT_SECS);
    let output = foreground(toolbox, &args.command, timeout).await?;
    feed_views(toolbox, &args.command);
    toolbox.maybe_spill(output, &format!("bash {:?}", args.command))
}

/// Asks `rtk rewrite` for the token-compressed form of the command. Exit 0 or 3 means rtk has one
/// and prints it; anything else (rtk missing, a timeout, an empty answer) leaves the command as
/// written.
async fn rtk_rewrite(command: &str) -> String {
    let trimmed = command.trim();
    if trimmed.is_empty() || trimmed == "rtk" || trimmed.starts_with("rtk ") {
        return command.to_string();
    }
    let asked = tokio::process::Command::new("rtk")
        .arg("rewrite")
        .arg(trimmed)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .output();
    let Ok(Ok(asked)) = tokio::time::timeout(REWRITE_TIMEOUT, asked).await else {
        return command.to_string();
    };
    if !matches!(asked.status.code(), Some(0) | Some(3)) {
        return command.to_string();
    }
    let rewritten = String::from_utf8_lossy(&asked.stdout).trim().to_string();
    if rewritten.is_empty() {
        command.to_string()
    } else {
        rewritten
    }
}

pub(crate) async fn foreground(
    toolbox: &Toolbox,
    command: &str,
    timeout_secs: u64,
) -> ToolResult<String> {
    let command = rtk_rewrite(command).await;
    let mut cmd = tokio::process::Command::new("sh");
    cmd.arg("-c")
        .arg(format!("{}exec 2>&1\n{command}", toolbox.env_prelude()))
        .current_dir(toolbox.root())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    if let Some(session_dir) = toolbox.spill_dir().parent() {
        cmd.env("EMBERLOOP_SESSION_DIR", session_dir);
    }
    #[cfg(unix)]
    cmd.process_group(0);
    let mut child = cmd.spawn()?;
    let pid = child.id().unwrap_or(0);
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| ToolError::msg("failed to capture bash stdout"))?;
    let cap = toolbox.max_output_bytes();
    let reader = tokio::spawn(async move {
        let mut buf = Vec::new();
        let mut limited = (&mut stdout).take(cap as u64 + 1);
        limited.read_to_end(&mut buf).await?;
        Ok::<Vec<u8>, std::io::Error>(buf)
    });
    let waited = tokio::time::timeout(Duration::from_secs(timeout_secs), child.wait()).await;
    let (status, timed_out) = match waited {
        Ok(status) => {
            // The shell is gone, but a child it detached may still run in its group.
            let status = status?;
            kill_group(pid);
            (Some(status), false)
        }
        Err(_) => {
            kill_group(pid);
            let _ = child.wait().await;
            (None, true)
        }
    };
    let bytes = reader
        .await
        .map_err(|e| ToolError::msg(format!("bash reader failed: {e}")))??;
    let mut text = String::from_utf8_lossy(&bytes).into_owned();
    if text.len() > cap {
        let mut cut = cap;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
        text.push_str("\n[output truncated]\n");
    }
    let header = if timed_out {
        format!("timeout after {timeout_secs}s (process group killed)\n")
    } else {
        format!("exit code: {}\n", exit_label(status))
    };
    Ok(format!("{header}{text}"))
}

fn exit_label(status: Option<std::process::ExitStatus>) -> String {
    match status.and_then(|s| s.code()) {
        Some(code) => code.to_string(),
        None => "terminated by signal".to_string(),
    }
}

pub(crate) fn kill_group(pid: u32) {
    #[cfg(unix)]
    if pid > 0 {
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
    #[cfg(not(unix))]
    let _ = pid;
}

fn feed_views(toolbox: &Toolbox, command: &str) {
    let mut parts = command.split_whitespace();
    let Some(program) = parts.next() else {
        return;
    };
    let rest: Vec<&str> = parts.collect();
    let view = match program {
        "cat" => whole_file(&rest),
        "head" => head_tail(&rest, false),
        "tail" => head_tail(&rest, true),
        "sed" => sed_view(&rest),
        "grep" => grep_view(&rest),
        _ => None,
    };
    if let Some((path, lines)) = view {
        record(toolbox, &path, lines);
    }
}

fn whole_file(args: &[&str]) -> Option<(String, Vec<u32>)> {
    let path = args.iter().find(|arg| !arg.starts_with('-'))?;
    Some((clean(path), Vec::new()))
}

fn head_tail(args: &[&str], from_tail: bool) -> Option<(String, Vec<u32>)> {
    let mut count = VIEW_LINES;
    let mut path = None;
    let mut index = 0;
    while index < args.len() {
        let arg = args[index];
        if arg == "-n" {
            count = args.get(index + 1)?.parse().ok()?;
            index += 2;
            continue;
        }
        if let Some(value) = arg.strip_prefix('-') {
            if let Ok(parsed) = value.parse() {
                count = parsed;
            }
        } else if path.is_none() {
            path = Some(clean(arg));
        }
        index += 1;
    }
    let path = path?;
    let text = String::from_utf8(fs::read(&path).ok()?).ok()?;
    let total = line_count(&text).max(1);
    let count = count.min(total);
    if from_tail {
        let start = total + 1 - count;
        Some((path, (start as u32..=total as u32).collect()))
    } else {
        Some((path, (1..=count as u32).collect()))
    }
}

fn sed_view(args: &[&str]) -> Option<(String, Vec<u32>)> {
    if args.first().copied() != Some("-n") {
        return None;
    }
    let expression = args.get(1)?.trim_matches(|c| c == '\'' || c == '"');
    let (start, end): (usize, usize) = match expression.split_once(',') {
        Some((start, end)) => (start.parse().ok()?, end.parse().ok()?),
        None => {
            let line = expression.parse().ok()?;
            (line, line)
        }
    };
    let path = clean(args.get(2)?);
    Some((path, (start as u32..=end.max(start) as u32).collect()))
}
fn grep_view(args: &[&str]) -> Option<(String, Vec<u32>)> {
    if args.len() < 3 || !args.contains(&"-n") {
        return None;
    }
    let pattern = args.iter().find(|arg| !arg.starts_with('-'))?;
    let path = clean(args.last()?);
    let regex = Regex::new(pattern).ok()?;
    let text = String::from_utf8(fs::read(&path).ok()?).ok()?;
    let lines = lines_of(&text)
        .into_iter()
        .enumerate()
        .filter(|(_, line)| regex.is_match(line))
        .map(|(index, _)| (index + 1) as u32)
        .collect();
    Some((path, lines))
}

fn clean(raw: &str) -> String {
    raw.trim_matches(|c| c == '\'' || c == '"').to_string()
}

fn record(toolbox: &Toolbox, raw: &str, lines: Vec<u32>) {
    let Ok(path) = toolbox.jail().resolve(raw) else {
        return;
    };
    let Ok(bytes) = fs::read(&path) else {
        return;
    };
    let Ok(text) = String::from_utf8(bytes.clone()) else {
        return;
    };
    let hash = content_hash(&bytes);
    let count = line_count(&text) as u32;
    if lines.is_empty() {
        toolbox.seen().record_range(&path, &hash, 1, count);
    } else {
        toolbox.seen().record(
            &path,
            &hash,
            lines.into_iter().filter(|line| *line <= count),
        );
    }
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn rtk_rewrite_leaves_rtk_empty_and_missing_rtk_alone() {
        // rtk is not installed in the check, and these guards hold whether it is or not.
        for command in ["", "  ", "rtk git status", "rtk bypass git diff"] {
            assert_eq!(super::rtk_rewrite(command).await, command);
        }
    }
}
