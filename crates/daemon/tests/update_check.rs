#![allow(clippy::unwrap_used, clippy::expect_used)]
use tempfile::tempdir;

/// `update --check` reports the daemon attempt and all six external binaries even when the
/// manifest is unreachable, and caches the probe under the explicit data root.
#[test]
fn update_check_reports_the_daemon_and_every_external_binary() {
    let dir = tempdir().unwrap();
    let empty_path = dir.path().join("path");
    std::fs::create_dir(&empty_path).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_emberloop-daemon"))
        .args([
            "--data-root",
            dir.path().to_str().unwrap(),
            "update",
            "--check",
            "--manifest",
            "http://127.0.0.1:1/latest.json",
        ])
        .env("PATH", &empty_path)
        .env("EMBERLOOP_AGY_BIN", dir.path().join("absent-agy"))
        .env("EMBERLOOP_PI_BIN", dir.path().join("absent-pi"))
        .env("EMBERLOOP_CLAUDE_BIN", dir.path().join("absent-claude"))
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "stdout: {stdout}");
    assert!(stdout.contains("update check failed"), "{stdout}");
    assert!(stdout.contains("external binaries:"), "{stdout}");
    for id in [
        "agy",
        "claude",
        "pi",
        "rtk",
        "figcraft",
        "codebase-memory-mcp",
    ] {
        assert!(
            stdout.lines().any(|line| line.trim_start().starts_with(id)),
            "{id} missing from {stdout}"
        );
    }
    assert!(dir.path().join("binaries-cache.json").is_file());
}
