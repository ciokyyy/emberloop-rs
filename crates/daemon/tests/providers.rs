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
    let agy = providers
        .iter()
        .find(|provider| provider.id == "agy")
        .unwrap();
    assert_eq!(agy.accounts[0].label, "Work");
    assert_eq!(agy.accounts[0].tier, "Free");
    assert!(agy.signed_in);
    assert!(agy.accounts[0].signed_in);

    login.sign_out("default").await.unwrap();
    assert!(!dir.path().join("accounts/agy/default").exists());
    let agy = login
        .providers()
        .await
        .into_iter()
        .find(|provider| provider.id == "agy")
        .unwrap();
    assert!(!agy.signed_in);
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
async fn opencode_key_and_chatgpt_are_stored_privately_and_listed() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempdir().unwrap();
    let keys = dir.path().join("keys.json");
    let auth = dir.path().join("pi/agent/auth.json");
    let login = AgyLogin::new(dir.path().join("accounts/agy"), dir.path().join("agy"))
        .with_keys(&keys)
        .with_pi_auth(&auth);

    let providers = login.providers().await;
    assert_eq!(
        providers.len(),
        4 + emberloop_core::providers::KEY_PROVIDERS.len()
    );
    let opencode = providers
        .iter()
        .find(|provider| provider.id == "opencode-go")
        .unwrap();
    assert!(!opencode.signed_in);
    let chatgpt = providers
        .iter()
        .find(|provider| provider.id == "openai-codex")
        .unwrap();
    assert!(!chatgpt.signed_in);
    let claude = providers
        .iter()
        .find(|provider| provider.id == "claude")
        .unwrap();
    assert!(!claude.signed_in);

    login.set_key("opencode-go", "  sk-oc-123  ").await.unwrap();
    let mode = std::fs::metadata(&keys).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    let stored: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&keys).unwrap()).unwrap();
    assert_eq!(stored["opencode-go"], "sk-oc-123");

    let providers = login.providers().await;
    let opencode = providers
        .iter()
        .find(|provider| provider.id == "opencode-go")
        .unwrap();
    assert!(opencode.signed_in);
    assert_eq!(opencode.accounts.len(), 1);
    assert_eq!(opencode.accounts[0].id, "opencode-go");

    std::fs::create_dir_all(auth.parent().unwrap()).unwrap();
    std::fs::write(
        &auth,
        r#"{"openai-codex":{"type":"oauth","access":"at","refresh":"rt","expires":1}}"#,
    )
    .unwrap();
    let chatgpt = login
        .providers()
        .await
        .into_iter()
        .find(|provider| provider.id == "openai-codex")
        .unwrap();
    assert!(chatgpt.signed_in);

    login.sign_out("openai-codex").await.unwrap();
    let chatgpt = login
        .providers()
        .await
        .into_iter()
        .find(|provider| provider.id == "openai-codex")
        .unwrap();
    assert!(!chatgpt.signed_in);

    login.sign_out("opencode-go").await.unwrap();
    let opencode = login
        .providers()
        .await
        .into_iter()
        .find(|provider| provider.id == "opencode-go")
        .unwrap();
    assert!(!opencode.signed_in);
}

#[tokio::test]
async fn registry_providers_take_a_pasted_key_and_unknown_ids_are_refused() {
    let dir = tempdir().unwrap();
    let keys = dir.path().join("keys.json");
    let login =
        AgyLogin::new(dir.path().join("accounts/agy"), dir.path().join("agy")).with_keys(&keys);
    let groq = |providers: Vec<api::providers::ProviderView>| {
        providers
            .into_iter()
            .find(|provider| provider.id == "groq")
            .unwrap()
    };
    let before = groq(login.providers().await);
    assert_eq!(
        (before.name.as_str(), before.auth.as_str()),
        ("Groq", "key")
    );
    assert!(!before.signed_in);

    login.set_key("groq", "gsk-1").await.unwrap();
    let after = groq(login.providers().await);
    assert!(after.signed_in);
    assert_eq!(after.accounts[0].id, "groq");

    let error = login.set_key("nope", "k").await.unwrap_err();
    assert_eq!(error.status, axum::http::StatusCode::NOT_FOUND);
    assert!(login.clear_key("nope").await.is_err());
    login.clear_key("groq").await.unwrap();
    assert!(!groq(login.providers().await).signed_in);
}
