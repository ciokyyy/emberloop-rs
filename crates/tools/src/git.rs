//! Read-only git inspection: only listing and inspection subcommands run, and
//! every flag that would write, configure or execute is refused, abbreviations
//! included (git accepts unambiguous long-option prefixes).
use std::process::Stdio;
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;
use tokio::io::AsyncReadExt;

use crate::{ToolError, ToolResult, Toolbox};

const ALLOWED: &[&str] = &[
    "blame",
    "branch",
    "cat-file",
    "diff",
    "grep",
    "log",
    "ls-files",
    "merge-base",
    "rev-parse",
    "show",
    "status",
    "tag",
];
const ALWAYS_ALLOWED: &[&str] = &[
    "diff",
    "log",
    "show",
    "blame",
    "status",
    "ls-files",
    "grep",
    "rev-parse",
    "merge-base",
    "cat-file",
];
const NO_EXT_DIFF: &[&str] = &["diff", "log", "show", "blame"];
const NO_TEXTCONV: &[&str] = &["diff", "log", "show", "blame", "grep"];
const REFUSED_LONGS: &[&str] = &[
    "output",
    "ext-diff",
    "textconv",
    "exec",
    "upload-pack",
    "open-files-in-pager",
    "filters",
    "show-signature",
];
const TEXT_EXACT_SUBS: &[&str] = &["diff", "log", "show", "grep"];
const BRANCH_MUTATING_SHORTS: &str = "cdDmMCfut";
const BRANCH_MUTATING_LONGS: &[&str] = &[
    "delete",
    "move",
    "copy",
    "edit-description",
    "create-reflog",
    "set-upstream-to",
    "unset-upstream",
    "track",
    "no-track",
    "force",
];
const TAG_MUTATING_SHORTS: &str = "dasmFuv";
const TAG_MUTATING_LONGS: &[&str] = &[
    "delete",
    "annotate",
    "sign",
    "local-user",
    "message",
    "file",
    "verify",
];
const DEFAULT_TIMEOUT_SECS: u64 = 120;
const MAX_TIMEOUT_SECS: u64 = 600;
const OUTPUT_CAP: usize = 1 << 20;

#[derive(Deserialize)]
struct GitArgs {
    args: Vec<String>,
    #[serde(default)]
    timeout: Option<u64>,
}

pub async fn run(toolbox: &Toolbox, args: &Value) -> ToolResult<String> {
    let GitArgs { args, timeout } = serde_json::from_value(args.clone())?;
    let argv = checked_argv(&args)?;
    let timeout = timeout
        .unwrap_or(DEFAULT_TIMEOUT_SECS)
        .clamp(1, MAX_TIMEOUT_SECS);
    let text = exec(toolbox, argv, timeout).await?;
    toolbox.maybe_spill(text, "git")
}

/// The full `git` argv after validation, or the refusal.
fn checked_argv(args: &[String]) -> ToolResult<Vec<String>> {
    let allowed = ALLOWED.join(", ");
    let Some(sub) = args.first() else {
        return Err(ToolError::msg(format!(
            "git args cannot be empty; allowed subcommands: {allowed}"
        )));
    };
    if let Some(flag) = refused_flag(args) {
        return Err(ToolError::msg(format!(
            "git flag {flag:?} is not allowed (refused flags: -c, -O, --output, --ext-diff, --textconv, --exec, --upload-pack, --open-files-in-pager, --filters, --show-signature, and their abbreviations)"
        )));
    }
    let rest = &args[1..];
    if sub == "branch" || sub == "tag" {
        check_list_only(sub, rest, &allowed)?;
    } else if !ALWAYS_ALLOWED.contains(&sub.as_str()) {
        return Err(ToolError::msg(format!(
            "git subcommand {sub:?} is not allowed; allowed subcommands: {allowed} (branch and tag list only)"
        )));
    }
    let mut argv = vec!["--no-pager".to_string(), sub.clone()];
    if NO_EXT_DIFF.contains(&sub.as_str()) {
        argv.push("--no-ext-diff".into());
    }
    if NO_TEXTCONV.contains(&sub.as_str()) {
        argv.push("--no-textconv".into());
    }
    argv.extend(rest.iter().cloned());
    Ok(argv)
}

fn is_short_bundle(arg: &str) -> bool {
    arg.len() > 1 && arg.starts_with('-') && !arg.starts_with("--")
}

/// Long flag name (before any `=`) of a `--flag`, or None.
fn long_name(arg: &str) -> Option<(&str, &str)> {
    let name = arg.strip_prefix("--")?;
    let (name, value) = name.split_once('=').unwrap_or((name, ""));
    (!name.is_empty()).then_some((name, value))
}

/// First refused flag in `args` (`args[0]` is the subcommand); scanning stops at `--`.
fn refused_flag(args: &[String]) -> Option<&str> {
    let sub = args.first().map_or("", String::as_str);
    for arg in args.iter().take_while(|a| *a != "--") {
        if arg == "-c" {
            return Some(arg);
        }
        if is_short_bundle(arg) {
            if sub == "grep" && arg[1..].contains('O') {
                return Some(arg);
            }
            continue;
        }
        let Some((name, value)) = long_name(arg) else {
            continue;
        };
        if name == "text" && TEXT_EXACT_SUBS.contains(&sub) {
            continue;
        }
        if REFUSED_LONGS.iter().any(|f| f.starts_with(name)) {
            return Some(arg);
        }
        // Formatting a signature runs the repo's gpg.program.
        if value.contains("%G") && ("format".starts_with(name) || "pretty".starts_with(name)) {
            return Some(arg);
        }
    }
    None
}

/// branch and tag run in list mode only: no operands, or a list flag and no mutating flag.
fn check_list_only(sub: &str, rest: &[String], allowed: &str) -> ToolResult<()> {
    let refusal = || {
        ToolError::msg(format!(
            "git {sub} is allowed for listing only (branch: --list, -a/--all, -r/--remotes; tag: --list/-l); allowed subcommands: {allowed} (branch and tag list only)"
        ))
    };
    let (shorts, longs, list_shorts, list_longs): (&str, &[&str], &str, &[&str]) = if sub == "tag" {
        (TAG_MUTATING_SHORTS, TAG_MUTATING_LONGS, "l", &["--list"])
    } else {
        (
            BRANCH_MUTATING_SHORTS,
            BRANCH_MUTATING_LONGS,
            "ar",
            &["--list", "--all", "--remotes", "--show-current"],
        )
    };
    let mut lists = false;
    for arg in rest.iter().filter(|a| *a != "--") {
        if is_short_bundle(arg) {
            // `-ad` parses as `-a -d`, and delete wins over listing.
            if arg[1..].chars().any(|c| shorts.contains(c)) {
                return Err(refusal());
            }
            lists |= arg[1..].chars().any(|c| list_shorts.contains(c));
        } else if let Some((name, _)) = long_name(arg) {
            if longs.iter().any(|m| m.starts_with(name)) {
                return Err(refusal());
            }
            lists |= list_longs.contains(&arg.as_str());
        }
    }
    if rest.is_empty() || lists {
        Ok(())
    } else {
        Err(refusal())
    }
}

/// Environment with the global and system git config cleared, so a hostile
/// config cannot add drivers, a pager or an optional index lock.
fn clean_env(command: &mut tokio::process::Command) {
    for (key, _) in std::env::vars_os() {
        let key = key.to_string_lossy();
        if matches!(
            key.as_ref(),
            "GIT_EXTERNAL_DIFF"
                | "GIT_CONFIG_PARAMETERS"
                | "GIT_OPTIONAL_LOCKS"
                | "GIT_PAGER"
                | "PAGER"
        ) || key.starts_with("GIT_CONFIG_")
        {
            command.env_remove(key.as_ref());
        }
    }
    command
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_PAGER", "cat")
        .env("PAGER", "cat")
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "core.fsmonitor")
        .env("GIT_CONFIG_VALUE_0", "false");
}

async fn exec(toolbox: &Toolbox, argv: Vec<String>, timeout_secs: u64) -> ToolResult<String> {
    let mut command = tokio::process::Command::new("git");
    command
        .arg("-C")
        .arg(toolbox.root())
        .args(&argv)
        .current_dir(toolbox.root())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    clean_env(&mut command);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command
        .spawn()
        .map_err(|e| ToolError::msg(format!("git could not run: {e}")))?;
    let pid = child.id().unwrap_or(0);
    let mut streams = (child.stdout.take(), child.stderr.take());
    let read = |stream: Option<Box<dyn tokio::io::AsyncRead + Unpin + Send>>| {
        tokio::spawn(async move {
            let mut buf = Vec::new();
            if let Some(stream) = stream {
                let _ = stream.take(OUTPUT_CAP as u64).read_to_end(&mut buf).await;
            }
            buf
        })
    };
    let out = read(streams.0.take().map(|s| Box::new(s) as _));
    let err = read(streams.1.take().map(|s| Box::new(s) as _));
    let waited = tokio::time::timeout(Duration::from_secs(timeout_secs), child.wait()).await;
    let status = match waited {
        Ok(status) => Some(status?),
        Err(_) => {
            crate::bash::kill_group(pid);
            let _ = child.wait().await;
            None
        }
    };
    let mut text = String::from_utf8_lossy(&out.await.unwrap_or_default()).into_owned();
    text.push_str(&String::from_utf8_lossy(&err.await.unwrap_or_default()));
    Ok(match status.and_then(|s| s.code()) {
        Some(0) => text,
        Some(1) if text.trim().is_empty() => "(no output, exit status 1)".into(),
        Some(code) => format!("{text}\n[exit status {code}]"),
        None => format!("{text}\n[timed out after {timeout_secs}s]"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(args: &[&str]) -> ToolResult<Vec<String>> {
        checked_argv(&args.iter().map(|a| a.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn inspection_subcommands_pass_with_safety_flags() {
        let argv = check(&["log", "--oneline", "-5"]).unwrap();
        assert_eq!(
            argv,
            [
                "--no-pager",
                "log",
                "--no-ext-diff",
                "--no-textconv",
                "--oneline",
                "-5"
            ]
        );
        assert_eq!(check(&["status"]).unwrap(), ["--no-pager", "status"]);
    }

    #[test]
    fn writing_subcommands_are_refused() {
        for sub in ["commit", "push", "reset", "checkout", "config"] {
            assert!(check(&[sub]).is_err(), "{sub}");
        }
        assert!(check(&[]).is_err());
    }

    #[test]
    fn dangerous_flags_and_abbreviations_are_refused() {
        for flags in [
            vec!["diff", "-c"],
            vec!["diff", "--output=x"],
            vec!["diff", "--out", "x"],
            vec!["log", "--ext-d"],
            vec!["show", "--textc"],
            vec!["grep", "-iOless", "x"],
            vec!["log", "--format=%GS"],
            vec!["cat-file", "--text", "HEAD"],
        ] {
            assert!(check(&flags).is_err(), "{flags:?}");
        }
        assert!(check(&["diff", "--text"]).is_ok());
        assert!(check(&["log", "--", "--output"]).is_ok());
    }

    #[test]
    fn branch_and_tag_list_only() {
        assert!(check(&["branch"]).is_ok());
        assert!(check(&["branch", "-a"]).is_ok());
        assert!(check(&["branch", "-av"]).is_ok());
        assert!(check(&["tag", "--list"]).is_ok());
        for flags in [
            vec!["branch", "newbranch"],
            vec!["branch", "-ad", "x"],
            vec!["branch", "--del", "x"],
            vec!["tag", "v1"],
            vec!["tag", "-a", "v1"],
            vec!["tag", "--annot", "v1"],
        ] {
            assert!(check(&flags).is_err(), "{flags:?}");
        }
    }
}
