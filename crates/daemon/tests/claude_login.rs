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

#[tokio::test]
async fn a_login_without_the_cli_fails_and_an_unknown_id_is_refused() {
    let dir = tempdir().unwrap();
    let login = ClaudeLogin::new(dir.path().join("nope"), dir.path().join("claude-homes"));
    let error = login.start("").await.err().unwrap();
    assert_eq!(error.status, axum::http::StatusCode::SERVICE_UNAVAILABLE);
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
