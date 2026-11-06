#![allow(clippy::unwrap_used, clippy::expect_used)]
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::time::{Duration, Instant};

use api::providers::{LoginError, ProviderLogin};
use emberloop_daemon::login::AgyLogin;
use tempfile::tempdir;
use tokio::sync::oneshot;

fn fake_agy(path: &Path, sign_in_url: &str, answer_auth: bool) {
    let answer = if answer_auth {
        "      sleep 1\n      mkdir -p \"$GEMINI_HOME/antigravity-acp\"\n      : > \"$GEMINI_HOME/antigravity-acp/acp_token.json\"\n      printf '{\"jsonrpc\":\"2.0\",\"id\":%s,\"result\":{\"_meta\":{\"antigravity\":{\"tier\":\"Free\"}}}}\\n' \"$id\"".to_string()
    } else {
        "      :".to_string()
    };
    let script = format!(
        "#!/bin/sh\n\
         [ \"$1\" = \"--uid=\" ] || exit 1\n\
         echo \"Open the following link to authenticate the ACP server: {sign_in_url}\" >&2\n\
         while IFS= read -r line; do\n\
           id=$(printf '%s' \"$line\" | sed -n 's/.*\"id\":\\([0-9][0-9]*\\).*/\\1/p')\n\
           case \"$line\" in\n\
             *authenticate*)\n{answer}\n\
               ;;\n\
             *)\n\
               printf '{{\"jsonrpc\":\"2.0\",\"id\":%s,\"result\":{{}}}}\\n' \"$id\"\n\
               ;;\n\
           esac\n\
         done\n"
    );
    std::fs::write(path, script).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn loopback_server() -> (String, oneshot::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (sender, receiver) = oneshot::channel();
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        let mut buffer = [0u8; 2048];
        let read = stream.read(&mut buffer).unwrap_or_default();
        let _ = sender.send(String::from_utf8_lossy(&buffer[..read]).to_string());
        let _ = stream.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nok");
    });
    (format!("http://127.0.0.1:{port}/oauth?code=ok"), receiver)
}

async fn wait_for_login(
    login: &AgyLogin,
    id: &str,
    predicate: impl Fn(&api::providers::LoginView) -> bool,
) -> api::providers::LoginView {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(view) = login.status(id).await {
            if predicate(&view) {
                return view;
            }
        }
        assert!(Instant::now() < deadline, "condition not reached");
        tokio::task::yield_now().await;
    }
}

#[tokio::test]
async fn the_sign_in_url_reaches_the_panel_and_the_pasted_address_finishes_the_login() {
    let dir = tempdir().unwrap();
    let (url, request) = loopback_server();
    let bin = dir.path().join("agy");
    fake_agy(&bin, &url, true);
    let login = AgyLogin::new(dir.path().join("accounts/agy"), &bin);

    let started = login.start("Work").await.unwrap();
    assert_eq!(started.state, "running");

    let view = wait_for_login(&login, &started.id, |view| !view.url.is_empty()).await;
    assert_eq!(view.url, url);

    // The pasted address completes the exchange on the fake server.
    let completed = login.complete(&started.id, &url).await.unwrap();
    assert!(matches!(completed.state.as_str(), "running" | "succeeded"));
    let request = request.await.unwrap();
    assert!(request.starts_with("GET /oauth"), "request was {request:?}");

    let finished = wait_for_login(&login, &started.id, |view| view.state == "succeeded").await;
    assert_eq!(finished.tier, "Free");

    let providers = login.providers().await;
    assert_eq!(providers.len(), 1);
    assert_eq!(providers[0].id, "agy");
    assert_eq!(providers[0].accounts[0].label, "Work");
    assert_eq!(providers[0].accounts[0].tier, "Free");
    assert!(providers[0].signed_in);
    assert!(providers[0].accounts[0].signed_in);

    login.sign_out("default").await.unwrap();
    assert!(!dir.path().join("accounts/agy/default").exists());
    assert!(!login.providers().await[0].signed_in);
}

#[tokio::test]
async fn a_second_login_answers_409_and_cancel_removes_the_account_home() {
    let dir = tempdir().unwrap();
    let bin = dir.path().join("agy");
    fake_agy(&bin, "http://127.0.0.1:1/oauth", false);
    let login = AgyLogin::new(dir.path().join("accounts/agy"), &bin);

    let started = login.start("").await.unwrap();
    let second = login.start("").await;
    assert!(matches!(
        second,
        Err(LoginError {
            status: axum::http::StatusCode::CONFLICT,
            ..
        })
    ));

    login.cancel(&started.id).await.unwrap();
    assert!(!dir.path().join("accounts/agy/default").exists());
    let view = login.status(&started.id).await.unwrap();
    assert_eq!(view.state, "canceled");
}

#[tokio::test]
async fn a_missing_server_answers_503() {
    let dir = tempdir().unwrap();
    let login = AgyLogin::new(dir.path().join("accounts/agy"), dir.path().join("nope"));
    let error = login.start("Work").await.err().unwrap();
    assert_eq!(error.status, axum::http::StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn opencode_key_is_stored_privately_and_listed_in_providers() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempdir().unwrap();
    let keys = dir.path().join("keys.json");
    let login =
        AgyLogin::new(dir.path().join("accounts/agy"), dir.path().join("agy")).with_keys(&keys);

    let providers = login.providers().await;
    assert_eq!(providers.len(), 2);
    assert_eq!(providers[1].id, "opencode-go");
    assert!(!providers[1].signed_in);

    login.set_key("opencode-go", "  sk-oc-123  ").await.unwrap();
    let mode = std::fs::metadata(&keys).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    let stored: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&keys).unwrap()).unwrap();
    assert_eq!(stored["opencode-go"], "sk-oc-123");

    let providers = login.providers().await;
    assert!(providers[1].signed_in);
    assert_eq!(providers[1].accounts.len(), 1);
    assert_eq!(providers[1].accounts[0].id, "opencode-go");

    login.sign_out("opencode-go").await.unwrap();
    assert!(!login.providers().await[1].signed_in);
}
