//! A scripted `pi --mode acp`: it answers every prompt with a JSON report of
//! what the harness gave it (argv, environment keys, per-Session config files).
#![allow(clippy::unwrap_used, clippy::expect_used)]
use agent_client_protocol::v1::schema::{
    AgentCapabilities, ContentBlock, ContentChunk, DeleteSessionRequest, DeleteSessionResponse,
    InitializeRequest, InitializeResponse, NewSessionRequest, NewSessionResponse, PromptRequest,
    PromptResponse, SessionConfigOption, SessionConfigSelectOption, SessionNotification,
    SessionUpdate, SetSessionConfigOptionRequest, SetSessionConfigOptionResponse, StopReason,
};
use agent_client_protocol::{Agent, Result, Stdio};
use std::sync::{Arc, Mutex};

fn read_home(file: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    std::fs::read_to_string(format!("{home}/.pi/agent/{file}")).unwrap_or_default()
}

#[tokio::main]
async fn main() -> Result<()> {
    let cwd = Arc::new(Mutex::new(String::new()));
    let picked = Arc::new(Mutex::new(String::new()));
    let (cwd_new, picked_set, cwd_prompt, picked_prompt) =
        (cwd.clone(), picked.clone(), cwd.clone(), picked.clone());
    Agent
        .v1()
        .name("fake-pi")
        .on_receive_request(
            async move |init: InitializeRequest, responder, _connection| {
                responder.respond(
                    InitializeResponse::new(init.protocol_version)
                        .agent_capabilities(AgentCapabilities::new()),
                )
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: NewSessionRequest, responder, _connection| {
                *cwd_new.lock().unwrap() = request.cwd.display().to_string();
                let models = ["glm-5", "kimi-k2"].map(|id| {
                    SessionConfigSelectOption::new(format!("opencode-go/{id}"), id.to_uppercase())
                });
                responder.respond(NewSessionResponse::new("pi-1").config_options(vec![
                    SessionConfigOption::select(
                        "model",
                        "Model",
                        "opencode-go/glm-5",
                        models.to_vec(),
                    ),
                ]))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: SetSessionConfigOptionRequest, responder, _connection| {
                *picked_set.lock().unwrap() = format!(
                    "{}={}",
                    request.config_id,
                    serde_json::to_string(&request.value).unwrap()
                );
                responder.respond(SetSessionConfigOptionResponse::new(Vec::new()))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: PromptRequest, responder, connection| {
                let report = serde_json::json!({
                    "args": std::env::args().skip(1).collect::<Vec<_>>(),
                    "env": std::env::vars().map(|(k, _)| k).collect::<Vec<_>>(),
                    "cwd": cwd_prompt.lock().unwrap().clone(),
                    "picked": picked_prompt.lock().unwrap().clone(),
                    "models": read_home("models.json"),
                    "mcp": read_home("mcp.json"),
                });
                connection.send_notification(SessionNotification::new(
                    request.session_id,
                    SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::from(
                        report.to_string(),
                    ))),
                ))?;
                responder.respond(PromptResponse::new(StopReason::EndTurn))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: DeleteSessionRequest, responder, connection| {
                connection.send_notification(SessionNotification::new(
                    request.session_id.clone(),
                    SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::from(
                        format!("deleted {}", request.session_id),
                    ))),
                ))?;
                responder.respond(DeleteSessionResponse::new())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_to(Stdio::new())
        .await
}
