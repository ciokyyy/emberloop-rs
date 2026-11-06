#![allow(clippy::cognitive_complexity)]
use std::convert::Infallible;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, Method, Response, StatusCode, Uri};
use axum::routing::any;
use axum::Router;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tracing::{error, info, warn};

const UPSTREAM_ORIGIN: &str = "https://daily-cloudcode-pa.googleapis.com";
const AGY_SERVER_BIN: &str = "/home/cioky/.local/share/emberloop/agy-acp/1.4.5/agy_acp_server.par";
const AGY_ACCOUNT_HOME: &str =
    "/home/cioky/.local/share/emberloop/agy-acp-accounts/acc_2c5257ad94a4";

#[derive(Default, Clone)]
struct ProxyState {
    inner: Arc<Mutex<ProxyInner>>,
}

#[derive(Default)]
struct ProxyInner {
    generation_requests: usize,
    captured_thought_signature: Option<String>,
    stored_splice_model_part: Option<Value>,
    stored_splice_user_part: Option<Value>,
    request1_initial_status: Option<u16>,
    swallowed_execution_status: Option<u16>,
    request2_resplice_status: Option<u16>,
    splice_applied_count: usize,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    info!("Starting Ticket 1 Spike: agy behind proxy");

    let proxy_state = ProxyState::default();
    let app = Router::new()
        .fallback(any(proxy_handler))
        .with_state(proxy_state.clone());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let proxy_port = listener.local_addr()?.port();
    let proxy_base = format!("http://127.0.0.1:{}", proxy_port);
    info!("Proxy listening on {}", proxy_base);

    let server_task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let spike_res = run_spike(&proxy_base, proxy_state.clone()).await;

    server_task.abort();

    match spike_res {
        Ok(()) => {
            info!("=== TICKET 1 SPIKE COMPLETED SUCCESSFULLY ===");
        }
        Err(e) => {
            error!("=== TICKET 1 SPIKE FAILED: {:#} ===", e);
            std::process::exit(1);
        }
    }

    Ok(())
}

async fn proxy_handler(
    State(state): State<ProxyState>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    req_body: Body,
) -> Result<Response<Body>, Infallible> {
    let path_and_query = uri
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or(uri.path());

    let is_generation = path_and_query.contains(":streamGenerateContent")
        || path_and_query.contains(":generateContent");

    let body_bytes = axum::body::to_bytes(req_body, 16 * 1024 * 1024)
        .await
        .unwrap_or_default();

    if !is_generation {
        // Forward control-plane path directly
        let client = reqwest::Client::new();
        let upstream_url = format!("{}{}", UPSTREAM_ORIGIN, path_and_query);

        let mut req_builder = client.request(method, &upstream_url);
        for (k, v) in &headers {
            if k.as_str().eq_ignore_ascii_case("host") {
                continue;
            }
            req_builder = req_builder.header(k.as_str(), v.as_bytes());
        }
        req_builder = req_builder.body(body_bytes);

        match req_builder.send().await {
            Ok(resp) => {
                let status = resp.status();
                let mut builder = Response::builder().status(status);
                for (k, v) in resp.headers() {
                    builder = builder.header(k.as_str(), v.as_bytes());
                }
                let stream = resp.bytes_stream();
                let body = Body::from_stream(stream);
                Ok(builder.body(body).unwrap())
            }
            Err(err) => {
                error!("Upstream control plane error: {:#}", err);
                Ok(Response::builder()
                    .status(StatusCode::BAD_GATEWAY)
                    .body(Body::from(format!("upstream error: {}", err)))
                    .unwrap())
            }
        }
    } else {
        handle_generation_request(state, path_and_query, headers, body_bytes).await
    }
}

async fn handle_generation_request(
    state: ProxyState,
    path_and_query: &str,
    headers: HeaderMap,
    body_bytes: axum::body::Bytes,
) -> Result<Response<Body>, Infallible> {
    let mut req_json: Value = match serde_json::from_slice(&body_bytes) {
        Ok(v) => v,
        Err(e) => {
            error!("Failed to parse generation request JSON: {:#}", e);
            return Ok(Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body(Body::from(format!("invalid json: {}", e)))
                .unwrap());
        }
    };

    let req_idx = {
        let mut inner = state.inner.lock().unwrap();
        inner.generation_requests += 1;
        inner.generation_requests
    };

    info!(
        "Intercepted generation request #{}: {}",
        req_idx, path_and_query
    );

    // Ensure the tool is injected on generation requests
    inject_spike_tool(&mut req_json);

    // If we have a stored splice and this is a subsequent request from agy, re-splice it
    let has_stored_splice = {
        let inner = state.inner.lock().unwrap();
        inner.stored_splice_model_part.is_some() && inner.stored_splice_user_part.is_some()
    };

    if has_stored_splice {
        info!("Applying byte-exact re-splice to request #{}", req_idx);
        apply_splice(&state, &mut req_json);
    }

    let client = reqwest::Client::new();
    let upstream_url = format!("{}{}", UPSTREAM_ORIGIN, path_and_query);

    let modified_body = serde_json::to_vec(&req_json).unwrap();
    let mut req_builder = client.post(&upstream_url);
    for (k, v) in &headers {
        if k.as_str().eq_ignore_ascii_case("host")
            || k.as_str().eq_ignore_ascii_case("content-length")
        {
            continue;
        }
        req_builder = req_builder.header(k.as_str(), v.as_bytes());
    }
    req_builder = req_builder.body(modified_body);

    let upstream_resp = match req_builder.send().await {
        Ok(r) => r,
        Err(err) => {
            error!("Upstream generation request error: {:#}", err);
            return Ok(Response::builder()
                .status(StatusCode::BAD_GATEWAY)
                .body(Body::from(format!("upstream error: {}", err)))
                .unwrap());
        }
    };

    let status = upstream_resp.status();
    info!("Upstream generation #{} status: {}", req_idx, status);

    {
        let mut inner = state.inner.lock().unwrap();
        if req_idx == 1 {
            inner.request1_initial_status = Some(status.as_u16());
        } else if req_idx == 2 {
            inner.request2_resplice_status = Some(status.as_u16());
        }
    }
    if !status.is_success() {
        let err_body = upstream_resp.text().await.unwrap_or_default();
        error!(
            "Upstream generation #{} failed (HTTP {}): {}",
            req_idx, status, err_body
        );
        return Ok(Response::builder()
            .status(status)
            .body(Body::from(err_body))
            .unwrap());
    }

    let sse_bytes = upstream_resp.bytes().await.unwrap_or_default();

    // Check if response contains our injected tool call
    let (tool_call_found, call_id, model_part) = extract_spike_tool_call(&sse_bytes);

    if tool_call_found && req_idx == 1 {
        info!("Intercepted tool call for spike_probe_tool in request #1!");
        let call_id = call_id.unwrap_or_else(|| "call_1".to_string());
        let model_part = model_part.unwrap();

        // Extract thoughtSignature
        if let Some(sig) = model_part.get("thoughtSignature").and_then(|s| s.as_str()) {
            info!(
                "Captured thoughtSignature: {}...",
                &sig[..sig.len().min(16)]
            );
            let mut inner = state.inner.lock().unwrap();
            inner.captured_thought_signature = Some(sig.to_string());
        }

        let user_part = json!({
            "functionResponse": {
                "name": "spike_probe_tool",
                "id": call_id,
                "response": {
                    "output": "PROBE_SUCCESS_CONFIRMED"
                }
            }
        });

        // Store splice byte-exact!
        {
            let mut inner = state.inner.lock().unwrap();
            inner.stored_splice_model_part = Some(model_part.clone());
            inner.stored_splice_user_part = Some(user_part.clone());
            info!("Stored byte-exact turn splice (model part + user response part)");
        }

        // SWALLOW THE PROXY TOOL CALL:
        // Execute request 2 directly to upstream Cloud Code with the spliced turn!
        info!("Swallowing proxy tool call from agy; executing request 2 directly upstream");

        let mut req2_json = req_json.clone();
        if let Some(contents) = req2_json
            .get_mut("request")
            .and_then(|r| r.get_mut("contents"))
            .and_then(|c| c.as_array_mut())
        {
            contents.push(json!({
                "role": "model",
                "parts": [model_part]
            }));
            contents.push(json!({
                "role": "user",
                "parts": [user_part]
            }));
        }

        let modified_req2_body = serde_json::to_vec(&req2_json).unwrap();
        let mut req2_builder = client.post(&upstream_url);
        for (k, v) in &headers {
            if k.as_str().eq_ignore_ascii_case("host")
                || k.as_str().eq_ignore_ascii_case("content-length")
            {
                continue;
            }
            req2_builder = req2_builder.header(k.as_str(), v.as_bytes());
        }
        req2_builder = req2_builder.body(modified_req2_body);

        let req2_resp = match req2_builder.send().await {
            Ok(r) => r,
            Err(err) => {
                error!("Upstream generation request 2 error: {:#}", err);
                return Ok(Response::builder()
                    .status(StatusCode::BAD_GATEWAY)
                    .body(Body::from(format!("upstream req2 error: {}", err)))
                    .unwrap());
            }
        };

        let req2_status = req2_resp.status();
        info!(
            "Upstream generation #2 (tool completion) status: {}",
            req2_status
        );
        {
            let mut inner = state.inner.lock().unwrap();
            inner.swallowed_execution_status = Some(req2_status.as_u16());
        }

        if !req2_status.is_success() {
            let err_body = req2_resp.text().await.unwrap_or_default();
            error!(
                "Upstream generation #2 failed (HTTP {}): {}",
                req2_status, err_body
            );
            return Ok(Response::builder()
                .status(req2_status)
                .body(Body::from(err_body))
                .unwrap());
        }

        let req2_sse_bytes = req2_resp.bytes().await.unwrap_or_default();
        info!(
            "Upstream generation #2 returned {} SSE bytes to forward to agy",
            req2_sse_bytes.len()
        );

        let mut builder = Response::builder().status(req2_status);
        builder = builder.header("Content-Type", "text/event-stream; charset=UTF-8");
        return Ok(builder.body(Body::from(req2_sse_bytes)).unwrap());
    }

    let mut builder = Response::builder().status(status);
    builder = builder.header("Content-Type", "text/event-stream; charset=UTF-8");
    Ok(builder.body(Body::from(sse_bytes)).unwrap())
}

fn inject_spike_tool(req_json: &mut Value) {
    let tool_decl = json!({
        "functionDeclarations": [
            {
                "name": "spike_probe_tool",
                "description": "Probe tool to test thought signature survival and proxy tool injection. Always call this when asked.",
                "parameters": {
                    "type": "OBJECT",
                    "properties": {
                        "echo": {
                            "type": "STRING",
                            "description": "String to echo back"
                        }
                    },
                    "required": ["echo"]
                }
            }
        ]
    });

    if let Some(request) = req_json.get_mut("request") {
        if let Some(tools) = request.get_mut("tools") {
            if let Some(tools_arr) = tools.as_array_mut() {
                let mut found = false;
                for t in tools_arr.iter_mut() {
                    if let Some(decls) = t.get_mut("functionDeclarations") {
                        if let Some(decls_arr) = decls.as_array_mut() {
                            if !decls_arr.iter().any(|d| {
                                d.get("name").and_then(|n| n.as_str()) == Some("spike_probe_tool")
                            }) {
                                decls_arr.push(tool_decl["functionDeclarations"][0].clone());
                            }
                            found = true;
                            break;
                        }
                    }
                }
                if !found {
                    tools_arr.push(tool_decl);
                }
            }
        } else {
            request["tools"] = json!([tool_decl]);
        }
    }
}

fn extract_spike_tool_call(sse_bytes: &[u8]) -> (bool, Option<String>, Option<Value>) {
    let text = String::from_utf8_lossy(sse_bytes);
    for line in text.lines() {
        if let Some(data) = line.strip_prefix("data:") {
            let data = data.trim();
            if data.is_empty() {
                continue;
            }
            if let Ok(v) = serde_json::from_str::<Value>(data) {
                if let Some(candidates) = v
                    .get("response")
                    .and_then(|r| r.get("candidates"))
                    .or_else(|| v.get("candidates"))
                    .and_then(|c| c.as_array())
                {
                    for candidate in candidates {
                        if let Some(parts) = candidate
                            .get("content")
                            .and_then(|c| c.get("parts"))
                            .and_then(|p| p.as_array())
                        {
                            for part in parts {
                                if let Some(fn_call) = part.get("functionCall") {
                                    if fn_call.get("name").and_then(|n| n.as_str())
                                        == Some("spike_probe_tool")
                                    {
                                        let call_id = fn_call
                                            .get("id")
                                            .and_then(|id| id.as_str())
                                            .map(|s| s.to_string());
                                        return (true, call_id, Some(part.clone()));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    (false, None, None)
}

fn apply_splice(state: &ProxyState, req_json: &mut Value) {
    let (model_part, user_part) = {
        let inner = state.inner.lock().unwrap();
        match (
            &inner.stored_splice_model_part,
            &inner.stored_splice_user_part,
        ) {
            (Some(m), Some(u)) => (m.clone(), u.clone()),
            _ => return,
        }
    };

    if let Some(contents) = req_json
        .get_mut("request")
        .and_then(|r| r.get_mut("contents"))
        .and_then(|c| c.as_array_mut())
    {
        // Check if splice already in contents
        let already_present = contents.iter().any(|turn| {
            turn.get("parts")
                .and_then(|p| p.as_array())
                .map(|parts| {
                    parts.iter().any(|part| {
                        part.get("functionCall")
                            .and_then(|f| f.get("name"))
                            .and_then(|n| n.as_str())
                            == Some("spike_probe_tool")
                    })
                })
                .unwrap_or(false)
        });

        if !already_present {
            let insert_idx = if contents.len() > 1 {
                contents.len() - 1
            } else {
                contents.len()
            };

            let model_turn = json!({
                "role": "model",
                "parts": [model_part]
            });

            let user_turn = json!({
                "role": "user",
                "parts": [user_part]
            });

            contents.insert(insert_idx, model_turn);
            contents.insert(insert_idx + 1, user_turn);

            let mut inner = state.inner.lock().unwrap();
            inner.splice_applied_count += 1;
            info!(
                "Re-spliced byte-exact turn pair at index {}. Total contents: {}",
                insert_idx,
                contents.len()
            );
        }
    }
}

async fn run_spike(proxy_base: &str, state: ProxyState) -> anyhow::Result<()> {
    let temp_work_dir = tempfile::tempdir()?;
    info!("Temporary work dir: {:?}", temp_work_dir.path());

    let mut child = Command::new(AGY_SERVER_BIN)
        .arg("--uid=")
        .current_dir(temp_work_dir.path())
        .env("GEMINI_HOME", AGY_ACCOUNT_HOME)
        .env("AGY_ACP_CCPA_BASE_URL", proxy_base)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;

    let stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");

    let mut writer = stdin;
    let mut reader = BufReader::new(stdout).lines();

    // 1. Send initialize
    info!("Step 1: Sending ACP initialize");
    let init_req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": 1,
            "clientCapabilities": {
                "fs": { "readTextFile": true, "writeTextFile": true },
                "terminal": false
            }
        }
    });
    writer
        .write_all(format!("{}\n", serde_json::to_string(&init_req)?).as_bytes())
        .await?;
    writer.flush().await?;

    let init_resp = reader.next_line().await?.expect("init response");
    info!("ACP initialize response: {}", init_resp);

    // 2. Send session/new
    info!("Step 2: Sending ACP session/new");
    let new_req = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "session/new",
        "params": {
            "cwd": temp_work_dir.path().to_str().unwrap(),
            "mcpServers": [],
            "_meta": {}
        }
    });
    writer
        .write_all(format!("{}\n", serde_json::to_string(&new_req)?).as_bytes())
        .await?;
    writer.flush().await?;

    let new_resp = reader.next_line().await?.expect("session/new response");
    info!("ACP session/new response: {}", new_resp);
    let new_val: Value = serde_json::from_str(&new_resp)?;
    let session_id = new_val["result"]["sessionId"]
        .as_str()
        .expect("sessionId in result");

    // 3. Set model to gemini-3.8-flash
    info!("Step 3: Setting model to gemini-3.8-flash");
    let model_req = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "session/set_config_option",
        "params": {
            "sessionId": session_id,
            "configId": "model",
            "value": "gemini-3.8-flash"
        }
    });
    writer
        .write_all(format!("{}\n", serde_json::to_string(&model_req)?).as_bytes())
        .await?;
    writer.flush().await?;

    let model_resp = reader.next_line().await?.expect("model response");
    info!("ACP set model response: {}", model_resp);

    // 4. Prompt 1: ask model to call the injected tool
    info!("Step 4: Sending Prompt 1 (requesting tool call)");
    let prompt1_req = json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "session/prompt",
        "params": {
            "sessionId": session_id,
            "prompt": [
                {
                    "type": "text",
                    "text": "You must call the tool spike_probe_tool with echo='probe_sig_test'. Do not reply with text only."
                }
            ]
        }
    });
    writer
        .write_all(format!("{}\n", serde_json::to_string(&prompt1_req)?).as_bytes())
        .await?;
    writer.flush().await?;

    // Handle ACP incoming messages (including permission requests if any)
    loop {
        tokio::select! {
            line = reader.next_line() => {
                match line? {
                    Some(l) => {
                        info!("ACP line: {}", l);
                        if let Ok(v) = serde_json::from_str::<Value>(&l) {
                            if let Some(id) = v.get("id") {
                                if id == &json!(4) {
                                    info!("Prompt 1 finished: {}", l);
                                    break;
                                }
                                if let Some(method) = v.get("method").and_then(|m| m.as_str()) {
                                    if method == "session/request_permission" {
                                        let opt_id = v.get("params")
                                            .and_then(|p| p.get("options"))
                                            .and_then(|o| o.as_array())
                                            .and_then(|arr| arr.first())
                                            .and_then(|first| first.get("optionId"))
                                            .and_then(|id| id.as_str())
                                            .unwrap_or("allow_once");

                                        let resp = json!({
                                            "jsonrpc": "2.0",
                                            "id": id,
                                            "result": {
                                                "outcome": "selected",
                                                "optionId": opt_id
                                            }
                                        });
                                        writer.write_all(format!("{}\n", serde_json::to_string(&resp)?).as_bytes()).await?;
                                        writer.flush().await?;
                                        info!("Answered permission request with {}", opt_id);
                                    }
                                }
                            }
                        }
                    }
                    None => break,
                }
            }
            _ = tokio::time::sleep(Duration::from_secs(60)) => {
                warn!("Prompt 1 timeout waiting for response");
                break;
            }
        }
    }

    // Check if thoughtSignature and splice were captured
    let (sig_captured, splice_stored) = {
        let inner = state.inner.lock().unwrap();
        (
            inner.captured_thought_signature.is_some(),
            inner.stored_splice_model_part.is_some(),
        )
    };
    info!(
        "After Prompt 1: sig_captured={}, splice_stored={}",
        sig_captured, splice_stored
    );

    // 5. Prompt 2: Send second prompt to trigger request 3 (where re-splice occurs)
    info!("Step 5: Sending Prompt 2 (testing re-splice byte-exact survival on next turn)");
    let prompt2_req = json!({
        "jsonrpc": "2.0",
        "id": 5,
        "method": "session/prompt",
        "params": {
            "sessionId": session_id,
            "prompt": [
                {
                    "type": "text",
                    "text": "Confirm you received the tool output and reply with exactly: SPIKE_VERIFIED_PASS"
                }
            ]
        }
    });
    writer
        .write_all(format!("{}\n", serde_json::to_string(&prompt2_req)?).as_bytes())
        .await?;
    writer.flush().await?;

    loop {
        tokio::select! {
            line = reader.next_line() => {
                match line? {
                    Some(l) => {
                        info!("ACP line: {}", l);
                        if let Ok(v) = serde_json::from_str::<Value>(&l) {
                            if let Some(id) = v.get("id") {
                                if id == &json!(5) {
                                    info!("Prompt 2 finished: {}", l);
                                    break;
                                }
                                if let Some(method) = v.get("method").and_then(|m| m.as_str()) {
                                    if method == "session/request_permission" {
                                        let opt_id = v.get("params")
                                            .and_then(|p| p.get("options"))
                                            .and_then(|o| o.as_array())
                                            .and_then(|arr| arr.first())
                                            .and_then(|first| first.get("optionId"))
                                            .and_then(|id| id.as_str())
                                            .unwrap_or("allow_once");

                                        let resp = json!({
                                            "jsonrpc": "2.0",
                                            "id": id,
                                            "result": {
                                                "outcome": "selected",
                                                "optionId": opt_id
                                            }
                                        });
                                        writer.write_all(format!("{}\n", serde_json::to_string(&resp)?).as_bytes()).await?;
                                        writer.flush().await?;
                                        info!("Answered permission request with {}", opt_id);
                                    }
                                }
                            }
                        }
                    }
                    None => break,
                }
            }
            _ = tokio::time::sleep(Duration::from_secs(60)) => {
                warn!("Prompt 2 timeout waiting for response");
                break;
            }
        }
    }

    let _ = child.kill().await;

    // Verify results
    let inner = state.inner.lock().unwrap();
    info!("==========================================");
    info!("--- SPIKE RESULTS SUMMARY ---");
    info!(
        "Total generation requests intercepted: {}",
        inner.generation_requests
    );
    info!(
        "Request 1 (initial) upstream status: {:?}",
        inner.request1_initial_status
    );
    info!(
        "Swallowed tool execution upstream status: {:?}",
        inner.swallowed_execution_status
    );
    info!(
        "Request 2 (re-spliced next turn) upstream status: {:?}",
        inner.request2_resplice_status
    );
    info!(
        "Captured thought signature: {:?}",
        inner.captured_thought_signature
    );
    info!(
        "Splice stored: {}",
        inner.stored_splice_model_part.is_some()
    );
    info!("Splices applied count: {}", inner.splice_applied_count);
    info!("==========================================");

    assert!(
        inner.request1_initial_status == Some(200),
        "Request 1 must succeed with HTTP 200"
    );
    assert!(
        inner.captured_thought_signature.is_some(),
        "Thought signature must be captured from Cloud Code"
    );
    assert!(
        inner.swallowed_execution_status == Some(200),
        "Swallowed execution with byte-exact spliced thoughtSignature must succeed with HTTP 200"
    );
    assert!(
        inner.request2_resplice_status == Some(200),
        "Next turn with re-spliced history must succeed with HTTP 200"
    );

    info!("THOUGHT SIGNATURES SURVIVED BYTE-EXACT RE-SPLICE: PASS");

    Ok(())
}
