//! A scripted stand-in for the real `claude` CLI: reads the same flags, prints
//! `system/init`, echoes stdin as stream events and answers SIGINT.
//! Its 2 ms poll is the fake's own loop, so the real-sleep lint does not apply.
#![allow(clippy::disallowed_methods)]
use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, TryRecvError};
use std::time::Duration;

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

extern "C" fn on_sigint(_: libc::c_int) {
    INTERRUPTED.store(true, Ordering::SeqCst);
}

fn main() {
    let mode = std::env::var("FAKE_CLAUDE_MODE").unwrap_or_else(|_| "ok".into());
    let dir = std::env::var("CLAUDE_CONFIG_DIR").unwrap_or_default();
    if let Ok(log) = std::env::var("FAKE_CLAUDE_LOG") {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let mut report = serde_json::json!({"args": args, "config_dir": dir});
        if let Ok(session) = std::env::var("FAKE_CLAUDE_SESSION") {
            report["session"] = session.into();
        }
        // Every run appends, so a respawn's flags stay readable.
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(log)
        {
            let _ = writeln!(file, "{report}");
        }
    }
    if mode == "no-init" {
        return;
    }
    if mode == "mcp-failed" {
        emit(
            r#"{"type":"system","subtype":"init","session_id":"fake-session","model":"fake-model","tools":[],"mcp_servers":[{"name":"el","status":"failed","source":"dynamic"}]}"#,
        );
        return;
    }
    // The first process dies on its first prompt, the second one runs normally.
    let crash = mode == "crash-once" && {
        let marker = format!("{dir}/crashed");
        let first = !std::path::Path::new(&marker).exists();
        if first {
            let _ = std::fs::write(&marker, "1");
        }
        first
    };
    emit(
        r#"{"type":"system","subtype":"init","session_id":"fake-session","model":"fake-model","tools":[],"mcp_servers":[{"name":"el","status":"connected","source":"dynamic"}]}"#,
    );
    let _ = unsafe { libc::signal(libc::SIGINT, on_sigint as *const () as usize) };
    // The reader thread keeps stdin from blocking the interrupt check.
    let (line_tx, line_rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if line_tx.send(line).is_err() {
                break;
            }
        }
    });
    let mut turns = 0;
    loop {
        // `cancel` is a bare SIGINT with no line behind it.
        if INTERRUPTED.swap(false, Ordering::SeqCst) {
            result("interrupted");
            continue;
        }
        let line = match line_rx.try_recv() {
            Ok(line) => line,
            Err(TryRecvError::Empty) => {
                std::thread::sleep(Duration::from_millis(2));
                continue;
            }
            Err(TryRecvError::Disconnected) => break,
        };
        if line.trim().is_empty() {
            continue;
        }
        turns += 1;
        if crash {
            std::process::exit(1);
        }
        let text = prompt_text(&line);
        if text.starts_with("tool:") {
            emit(&format!(
                r#"{{"type":"assistant","message":{{"role":"assistant","content":[{{"type":"tool_use","id":"tool-{turns}","name":"mcp__el__todo","input":{{"text":{}}}}}]}}}}"#,
                serde_json::to_string(&text).unwrap_or_default()
            ));
        } else if text.starts_with("spin:") {
            // Stays in the turn until SIGINT arrives.
            delta("started");
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while !INTERRUPTED.swap(false, Ordering::SeqCst) && std::time::Instant::now() < deadline
            {
                std::thread::sleep(Duration::from_millis(2));
            }
            result("interrupted");
            continue;
        } else if text.starts_with("wait:") {
            delta("started");
            // A mid-turn message: the next stdin line arrives before the result.
            let queued = line_rx
                .recv_timeout(Duration::from_secs(5))
                .map(|line| prompt_text(&line))
                .unwrap_or_default();
            delta(&format!("steer:{queued}"));
        } else {
            delta(&format!("echo:{text}"));
        }
        result("end_turn");
    }
}

fn prompt_text(line: &str) -> String {
    serde_json::from_str::<serde_json::Value>(line)
        .ok()
        .and_then(|value| {
            value["message"]["content"][0]["text"]
                .as_str()
                .map(str::to_string)
        })
        .unwrap_or_default()
}

fn emit(line: &str) {
    let mut out = std::io::stdout();
    if writeln!(out, "{line}").is_err() {
        std::process::exit(1);
    }
    let _ = out.flush();
}

fn delta(text: &str) {
    emit(&format!(
        r#"{{"type":"stream_event","event":{{"type":"content_block_delta","index":0,"delta":{{"type":"text_delta","text":{}}}}}}}"#,
        serde_json::to_string(text).unwrap_or_default()
    ));
}

fn result(stop_reason: &str) {
    emit(&format!(
        r#"{{"type":"result","subtype":"success","is_error":false,"stop_reason":"{stop_reason}","usage":{{"input_tokens":1,"output_tokens":2}}}}"#
    ));
}
