#![allow(clippy::unwrap_used, clippy::expect_used)]
use emberloop_daemon::claude_login::ClaudeLogin;
use serde_json::json;
use tempfile::tempdir;

fn account(homes: &std::path::Path) -> std::path::PathBuf {
    let home = homes.join("default");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(
        home.join(".credentials.json"),
        json!({"claudeAiOauth": {"accessToken": "sk-ant-oat-1", "subscriptionType": "pro"}})
            .to_string(),
    )
    .unwrap();
    std::fs::write(
        home.join(".claude.json"),
        json!({"oauthAccount": {"emailAddress": "dev@example.com"}}).to_string(),
    )
    .unwrap();
    home
}

#[tokio::test]
async fn an_account_home_is_signed_in_only_while_it_holds_a_token() {
    let dir = tempdir().unwrap();
    let homes = dir.path().join("claude-homes");
    let login = ClaudeLogin::new(dir.path().join("claude"), &homes);

    let provider = login.provider().await;
    assert_eq!(provider.id, "claude");
    assert_eq!(provider.auth, "login");
    assert!(!provider.signed_in);
    assert!(provider.accounts.is_empty());

    let home = account(&homes);
    assert!(login.signed_in());
    let provider = login.provider().await;
    assert!(provider.signed_in);
    assert_eq!(provider.accounts.len(), 1);
    assert_eq!(provider.accounts[0].label, "dev@example.com");
    assert_eq!(provider.accounts[0].tier, "pro");

    login.sign_out().await.unwrap();
    assert!(!login.signed_in());
    assert!(home.join(".claude.json").exists());
}

/// A stand-in for `claude auth login`: it either waits for a code or writes a credential and exits.
fn fake_cli(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join("claude");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[tokio::test]
async fn a_running_login_is_adopted_instead_of_refused() {
    let dir = tempdir().unwrap();
    let homes = dir.path().join("claude-homes");
    let login = ClaudeLogin::new(fake_cli(dir.path(), "sleep 30"), &homes);
    let started = login.start("").await.unwrap();
    let again = login.start("").await.unwrap();
    assert_eq!(again.id, started.id);
    login.cancel(&started.id).await.unwrap();
}

#[tokio::test]
#[allow(clippy::disallowed_methods)] // polls the spawned CLI, no fake clock to drive it
async fn a_printed_token_is_stored_and_never_shown() {
    let dir = tempdir().unwrap();
    let homes = dir.path().join("claude-homes");
    let cli = fake_cli(
        dir.path(),
        "printf '\\033]8;;https://claude.com/cai/oauth/authorize?code=true\\033\\\\link\\n'; sleep 1; printf 'Token: \\033[1msk-ant-oat01-abc_123\\033[0m\\n'; sleep 30",
    );
    let login = ClaudeLogin::new(cli, &homes);
    let started = login.start("").await.unwrap();
    for _ in 0..100 {
        let view = login.status(&started.id).await.unwrap();
        if view.state != "running" {
            assert_eq!(view.state, "succeeded", "{view:?}");
            assert_eq!(view.url, "https://claude.com/cai/oauth/authorize?code=true");
            assert!(!view.output.contains("sk-ant-oat"), "{view:?}");
            let saved = std::fs::read_to_string(homes.join("default/oauth-token")).unwrap();
            assert_eq!(saved, "sk-ant-oat01-abc_123");
            assert!(login.signed_in());
            login.sign_out().await.unwrap();
            assert!(!login.signed_in());
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("the login never finished");
}

#[tokio::test]
#[allow(clippy::disallowed_methods)] // polls the spawned CLI, no fake clock to drive it
async fn a_login_without_the_cli_fails_and_an_unknown_id_is_refused() {
    let dir = tempdir().unwrap();
    let login = ClaudeLogin::new(dir.path().join("nope"), dir.path().join("claude-homes"));
    let started = login.start("").await.unwrap();
    for _ in 0..50 {
        if login.status(&started.id).await.unwrap().state == "failed" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert_eq!(login.status(&started.id).await.unwrap().state, "failed");
    assert!(login.status("claudelogin_1").await.is_none());
    let error = login.complete("claudelogin_1", "code").await.err().unwrap();
    assert_eq!(error.status, axum::http::StatusCode::NOT_FOUND);
}

/// Live: the real CLI prints a sign-in URL; LIVE_CLAUDE_BIN points at it.
#[tokio::test]
#[ignore]
#[allow(clippy::disallowed_methods)] // polls the real CLI, no fake clock to drive it
async fn the_real_cli_prints_a_sign_in_url() {
    let bin = std::env::var("LIVE_CLAUDE_BIN").unwrap_or_else(|_| "claude".into());
    let dir = tempdir().unwrap();
    let login = ClaudeLogin::new(bin, dir.path().join("claude-homes"));
    let started = login.start("").await.unwrap();
    for _ in 0..60 {
        let view = login.status(&started.id).await.unwrap();
        if !view.url.is_empty() {
            assert!(
                view.url
                    .starts_with("https://claude.com/cai/oauth/authorize"),
                "{view:?}"
            );
            login.cancel(&started.id).await.unwrap();
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    panic!("the claude CLI printed no sign-in URL in 30 s");
}
