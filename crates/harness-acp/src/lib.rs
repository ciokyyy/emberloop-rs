#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::v1::schema::{
    CancelNotification, ClientCapabilities, ClientSessionCapabilities, ContentBlock,
    DeleteSessionRequest, ImageContent, Implementation, InitializeRequest, LoadSessionRequest,
    McpServer, NewSessionRequest, PermissionOptionKind, PromptRequest, RequestPermissionOutcome,
    RequestPermissionRequest, RequestPermissionResponse, SelectedPermissionOutcome,
    SessionConfigOptionValue, SessionNotification, SessionUpdate, SetSessionConfigOptionRequest,
    StopReason, TextContent, ToolCallStatus,
};
use agent_client_protocol::v1::ConnectionTo;
use agent_client_protocol::{AcpAgent, AcpAgentConfig, Agent, Client};
use async_trait::async_trait;
use harness::{EventStream, HarnessError, HarnessEvent, HarnessSession, PromptImage};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;
use tracing::debug;

/// Everything a harness decides about its ACP agent; the driver does the rest.
pub struct AcpLaunch {
    pub config: AcpAgentConfig,
    pub cwd: PathBuf,
    pub mcp_servers: Vec<McpServer>,
    pub meta: Option<serde_json::Map<String, serde_json::Value>>,
    /// Picked with `session/set_config_option`; the agent default applies when unset.
    pub model: Option<String>,
    /// Picked with `session/set_config_option` (`thought_level`) when non-empty.
    pub thought_level: Option<String>,
    /// The name sent in `initialize`; some servers gate models on it.
    pub client_name: &'static str,
    /// Names the agent in error messages.
    pub label: &'static str,
    /// False when the proxy already publishes the agent's reasoning from the model wire.
    pub forward_thoughts: bool,
    /// The agent's id for a conversation from an earlier run. The driver loads
    /// it, without replaying its history, and falls back to a new one.
    pub resume: Option<String>,
}

enum Command {
    Prompt(Vec<ContentBlock>),
    Cancel,
    Resume(String),
    Forget(String),
    SetThoughtLevel(String),
    Configure(String, Option<String>, oneshot::Sender<Result<(), String>>),
}

pub struct AcpSession {
    /// The `configOptions` the agent returned from `session/new` (a JSON array).
    pub config_options: serde_json::Value,
    token: String,
    resumed: bool,
    commands: mpsc::UnboundedSender<Command>,
    events: broadcast::Sender<HarnessEvent>,
}

/// How long `session/new` may take. agy makes three Google calls (token refresh, onboarding,
/// model list) on a cold start, 7s each on a slow network, so 10s was never enough.
const SESSION_READY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Spawn the agent, open one ACP session on it and return once it is ready.
pub async fn start(launch: AcpLaunch, session_id: &str) -> Result<AcpSession, HarnessError> {
    let label = launch.label;
    let (events, _) = broadcast::channel(256);
    let (commands, mut receiver) = mpsc::unbounded_channel();
    let (ready, mut ready_rx) = mpsc::channel(1);
    let event_sink = events.clone();
    let connection_sink = event_sink.clone();
    let ready_sender = ready.clone();
    let AcpLaunch {
        config,
        cwd,
        mcp_servers,
        meta,
        model,
        thought_level,
        client_name,
        resume,
        forward_thoughts,
        ..
    } = launch;
    // The agent replays a loaded conversation as notifications; those are
    // history the feed already has, not news.
    let replaying = Arc::new(AtomicBool::new(false));

    tokio::spawn(async move {
        let result = Client
            .v1()
            .name("emberloop")
            .on_receive_notification(
                {
                    let event_sink = event_sink.clone();
                    let replaying = Arc::clone(&replaying);
                    async move |notification: SessionNotification, _connection| {
                        if replaying.load(Ordering::SeqCst) {
                            return Ok(());
                        }
                        match notification_event(notification.update) {
                            Some(HarnessEvent::ThoughtChunk(_)) if !forward_thoughts => {}
                            Some(event) => {
                                let _ = event_sink.send(event);
                            }
                            None => {}
                        }
                        Ok(())
                    }
                },
                agent_client_protocol::on_receive_notification!(),
            )
            .on_receive_request(
                async move |request: RequestPermissionRequest, responder, _connection| {
                    let option = [
                        PermissionOptionKind::AllowOnce,
                        PermissionOptionKind::AllowAlways,
                    ]
                    .iter()
                    .find_map(|kind| {
                        request
                            .options
                            .iter()
                            .find(|option| option.kind == *kind)
                            .map(|option| option.option_id.clone())
                    });
                    let outcome = option.map_or(RequestPermissionOutcome::Cancelled, |id| {
                        RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(id))
                    });
                    responder.respond(RequestPermissionResponse::new(outcome))
                },
                agent_client_protocol::on_receive_request!(),
            )
            .connect_with(
                AcpAgent::new(config),
                move |connection: ConnectionTo<Agent>| async move {
                    let initialized = connection
                        .send_request(initialize_request(client_name))
                        .block_task()
                        .await?;
                    debug!(agent = ?initialized.agent_info, "{label} initialized");

                    let loaded = match resume.as_deref() {
                        Some(token) => {
                            try_load(&connection, token, &cwd, mcp_servers.clone(), &replaying)
                                .await
                        }
                        None => None,
                    };
                    // Kept alive for the life of the connection when a new
                    // session had to be opened.
                    let mut opened_new = None;
                    let (session_id, options, resumed) = match loaded {
                        Some(options) => (
                            agent_client_protocol::v1::schema::SessionId::new(
                                resume.clone().unwrap_or_default(),
                            ),
                            options,
                            true,
                        ),
                        None => {
                            let mut request = NewSessionRequest::new(&cwd).mcp_servers(mcp_servers);
                            request.meta = meta;
                            let started = connection
                                .build_session_from(request)
                                .block_task()
                                .start_session()
                                .await?;
                            let opened = (
                                started.response.session_id.clone(),
                                serde_json::to_value(&started.response.config_options)
                                    .unwrap_or_default(),
                                false,
                            );
                            opened_new = Some(started);
                            opened
                        }
                    };
                    if let Some(model) = model {
                        let picked = connection
                            .send_request(SetSessionConfigOptionRequest::new(
                                session_id.clone(),
                                "model",
                                SessionConfigOptionValue::value_id(model.clone()),
                            ))
                            .block_task()
                            .await;
                        if let Err(error) = picked {
                            let _ = ready_sender
                                .try_send(Err(format!("{label} refused model {model}: {error}")));
                            return Ok(());
                        }
                    }
                    if let Some(level) = thought_level.filter(|l| !l.trim().is_empty()) {
                        let _ = connection
                            .send_request(SetSessionConfigOptionRequest::new(
                                session_id.clone(),
                                "thought_level",
                                SessionConfigOptionValue::value_id(level),
                            ))
                            .block_task()
                            .await;
                    }
                    let _ = ready_sender.try_send(Ok((session_id.clone(), options, resumed)));

                    while let Some(command) = receiver.recv().await {
                        match command {
                            Command::Prompt(content) => {
                                let connection = connection.clone();
                                let session_id = session_id.clone();
                                let event_sink = connection_sink.clone();
                                tokio::spawn(async move {
                                    match connection
                                        .send_request(PromptRequest::new(session_id, content))
                                        .block_task()
                                        .await
                                    {
                                        Ok(response) => {
                                            let _ = event_sink.send(HarnessEvent::TurnFinished {
                                                stop_reason: stop_reason(response.stop_reason),
                                                usage: None,
                                            });
                                        }
                                        Err(error) => {
                                            let _ = event_sink
                                                .send(HarnessEvent::Error(error.to_string()));
                                        }
                                    }
                                });
                            }
                            Command::Cancel => {
                                connection.send_notification(CancelNotification::new(
                                    session_id.clone(),
                                ))?;
                            }
                            Command::Resume(token) => {
                                connection
                                    .send_request(LoadSessionRequest::new(token, &cwd))
                                    .block_task()
                                    .await?;
                            }
                            Command::Forget(token) => {
                                connection
                                    .send_request(DeleteSessionRequest::new(token))
                                    .block_task()
                                    .await?;
                            }
                            Command::Configure(model, effort, reply) => {
                                let result = configure(
                                    &connection,
                                    &session_id.to_string(),
                                    &model,
                                    effort.as_deref(),
                                )
                                .await;
                                let _ = reply.send(result);
                            }
                            Command::SetThoughtLevel(level) => {
                                let _ = connection
                                    .send_request(SetSessionConfigOptionRequest::new(
                                        session_id.clone(),
                                        "thought_level",
                                        SessionConfigOptionValue::value_id(level),
                                    ))
                                    .block_task()
                                    .await;
                            }
                        }
                    }
                    drop(opened_new);
                    Ok(())
                },
            )
            .await;

        if let Err(error) = result {
            let _ = ready.try_send(Err(error.to_string()));
            let _ = event_sink.send(HarnessEvent::Error(error.to_string()));
        }
    });

    let (acp_session, config_options, resumed) =
        tokio::time::timeout(SESSION_READY_TIMEOUT, ready_rx.recv())
            .await
            .map_err(|_| {
                HarnessError::Protocol(format!("{label} did not create session {session_id}"))
            })?
            .ok_or_else(|| HarnessError::Protocol(format!("{label} connection closed")))?
            .map_err(HarnessError::Protocol)?;

    debug!(session_id = %acp_session, "{label} session ready");
    Ok(AcpSession {
        config_options,
        token: acp_session.to_string(),
        resumed,
        commands,
        events,
    })
}

/// Load a stored conversation. `None` means the agent does not have it any
/// more, and the caller opens a new one.
async fn try_load(
    connection: &ConnectionTo<Agent>,
    token: &str,
    cwd: &Path,
    mcp_servers: Vec<McpServer>,
    replaying: &AtomicBool,
) -> Option<serde_json::Value> {
    replaying.store(true, Ordering::SeqCst);
    let loaded = connection
        .send_request(LoadSessionRequest::new(token.to_string(), cwd).mcp_servers(mcp_servers))
        .block_task()
        .await;
    replaying.store(false, Ordering::SeqCst);
    let response = loaded.ok()?;
    Some(serde_json::to_value(&response.config_options).unwrap_or_default())
}

async fn configure(
    connection: &ConnectionTo<Agent>,
    session: &str,
    model: &str,
    effort: Option<&str>,
) -> Result<(), String> {
    for (id, value) in [("model", Some(model)), ("thought_level", effort)] {
        if let Some(value) = value {
            connection
                .send_request(SetSessionConfigOptionRequest::new(
                    session.to_string(),
                    id,
                    SessionConfigOptionValue::value_id(value.to_string()),
                ))
                .block_task()
                .await
                .map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

fn initialize_request(client_name: &str) -> InitializeRequest {
    InitializeRequest::new(ProtocolVersion::V1)
        .client_capabilities(ClientCapabilities::new().session(ClientSessionCapabilities::new()))
        .client_info(Implementation::new(client_name, "0.0.0"))
}

fn stop_reason(reason: StopReason) -> Option<String> {
    serde_json::to_value(reason)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
}

fn notification_event(update: SessionUpdate) -> Option<HarnessEvent> {
    match update {
        SessionUpdate::AgentMessageChunk(chunk) => match chunk.content {
            ContentBlock::Text(text) => Some(HarnessEvent::AssistantChunk(text.text)),
            _ => None,
        },
        SessionUpdate::AgentThoughtChunk(chunk) => match chunk.content {
            ContentBlock::Text(text) => Some(HarnessEvent::ThoughtChunk(text.text)),
            _ => None,
        },
        SessionUpdate::ToolCall(call) => {
            let name = call.name.unwrap_or_else(|| call.title.clone());
            let done = matches!(
                call.status,
                ToolCallStatus::Completed | ToolCallStatus::Failed
            );
            if done {
                Some(HarnessEvent::ToolFinished {
                    id: call.tool_call_id.to_string(),
                    output: tool_output(call.raw_output, call.content),
                    status: Some(status_name(call.status)),
                })
            } else {
                Some(HarnessEvent::ToolStarted {
                    id: call.tool_call_id.to_string(),
                    name,
                    input: call.raw_input.unwrap_or_else(|| serde_json::json!({})),
                })
            }
        }
        SessionUpdate::ToolCallUpdate(update) => {
            let done = matches!(
                update.fields.status,
                Some(ToolCallStatus::Completed | ToolCallStatus::Failed)
            );
            if !done {
                let raw_input = update.fields.raw_input?;
                let name = update
                    .fields
                    .name
                    .or(update.fields.title)
                    .unwrap_or_default();
                return Some(HarnessEvent::ToolStarted {
                    id: update.tool_call_id.to_string(),
                    name,
                    input: raw_input,
                });
            }
            Some(HarnessEvent::ToolFinished {
                id: update.tool_call_id.to_string(),
                output: tool_output(
                    update.fields.raw_output,
                    update.fields.content.unwrap_or_default(),
                ),
                status: update.fields.status.map(status_name),
            })
        }
        _ => None,
    }
}

fn status_name(status: ToolCallStatus) -> String {
    match status {
        ToolCallStatus::Failed => "failed".to_string(),
        _ => "completed".to_string(),
    }
}

fn tool_output(
    raw: Option<serde_json::Value>,
    content: Vec<agent_client_protocol::v1::schema::ToolCallContent>,
) -> serde_json::Value {
    let value = serde_json::to_value(content).unwrap_or(serde_json::Value::Null);
    let texts: Vec<&str> = value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| item.pointer("/content/text").and_then(|t| t.as_str()))
        .collect();
    if !texts.is_empty() {
        return serde_json::Value::String(texts.join("\n"));
    }
    if let Some(raw) = raw {
        if let Some(unwrapped) = unwrap_raw_output(&raw) {
            return serde_json::Value::String(unwrapped);
        }
        return raw;
    }
    value
}

fn unwrap_raw_output(raw: &serde_json::Value) -> Option<String> {
    if let Some(s) = raw.as_str() {
        return Some(s.to_string());
    }
    if let Some(arr) = raw.get("content").and_then(serde_json::Value::as_array) {
        let parts: Vec<&str> = arr
            .iter()
            .filter_map(|item| item.get("text").and_then(serde_json::Value::as_str))
            .collect();
        if !parts.is_empty() {
            return Some(parts.join("\n"));
        }
    }
    for key in ["output", "result", "text"] {
        if let Some(s) = raw.get(key).and_then(serde_json::Value::as_str) {
            return Some(s.to_string());
        }
    }
    None
}

#[async_trait]
impl HarnessSession for AcpSession {
    async fn send(&self, prompt: &str, images: Vec<PromptImage>) -> Result<(), HarnessError> {
        let mut content = Vec::with_capacity(images.len() + 1);
        for image in images {
            content.push(ContentBlock::Image(ImageContent::new(
                image.data_base64,
                image.mime_type,
            )));
        }
        content.push(ContentBlock::Text(TextContent::new(prompt)));
        self.commands
            .send(Command::Prompt(content))
            .map_err(|_| HarnessError::Process("ACP connection closed".into()))
    }

    fn events(&self) -> EventStream {
        Box::pin(BroadcastStream::new(self.events.subscribe()).filter_map(|event| event.ok()))
    }

    async fn cancel(&self) -> Result<(), HarnessError> {
        self.commands
            .send(Command::Cancel)
            .map_err(|_| HarnessError::Process("ACP connection closed".into()))
    }

    fn resume_token(&self) -> Option<String> {
        Some(self.token.clone())
    }

    fn resumed(&self) -> bool {
        self.resumed
    }

    async fn resume(&self, token: &str) -> Result<(), HarnessError> {
        self.commands
            .send(Command::Resume(token.to_string()))
            .map_err(|_| HarnessError::Process("ACP connection closed".into()))
    }

    async fn wake(&self, text: &str, images: Vec<PromptImage>) -> Result<(), HarnessError> {
        self.send(text, images).await
    }

    async fn forget(&self, resume_token: &str) -> Result<(), HarnessError> {
        self.commands
            .send(Command::Forget(resume_token.to_string()))
            .map_err(|_| HarnessError::Process("ACP connection closed".into()))
    }

    async fn configure_model(&self, model: &str, effort: Option<&str>) -> Result<(), HarnessError> {
        let (reply, received) = oneshot::channel();
        self.commands
            .send(Command::Configure(
                model.to_string(),
                effort.map(str::to_string),
                reply,
            ))
            .map_err(|_| HarnessError::Process("ACP connection closed".into()))?;
        tokio::time::timeout(std::time::Duration::from_secs(10), received)
            .await
            .map_err(|_| HarnessError::Protocol("model change timed out".into()))?
            .map_err(|_| HarnessError::Process("ACP connection closed".into()))?
            .map_err(HarnessError::Protocol)
    }

    async fn set_thought_level(&self, level: &str) -> Result<(), HarnessError> {
        self.commands
            .send(Command::SetThoughtLevel(level.to_string()))
            .map_err(|_| HarnessError::Process("ACP connection closed".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_carries_the_client_name() {
        let request = initialize_request("zed");
        assert_eq!(request.client_info.unwrap().name, "zed");
    }

    #[test]
    fn tool_updates_become_harness_events() {
        let started = notification_event(SessionUpdate::ToolCall(
            agent_client_protocol::v1::schema::ToolCall::new("call-1", "read file")
                .name("read")
                .raw_input(serde_json::json!({"path": "a"})),
        ));
        assert!(matches!(
            started,
            Some(HarnessEvent::ToolStarted { ref id, ref name, .. })
                if id == "call-1" && name == "read"
        ));

        let finished = notification_event(SessionUpdate::ToolCallUpdate(
            agent_client_protocol::v1::schema::ToolCallUpdate::new(
                "call-1",
                agent_client_protocol::v1::schema::ToolCallUpdateFields::new()
                    .status(ToolCallStatus::Completed)
                    .raw_output(serde_json::json!({"ok": true})),
            ),
        ));
        assert!(matches!(
            finished,
            Some(HarnessEvent::ToolFinished { ref id, .. }) if id == "call-1"
        ));
    }

    #[test]
    fn tool_output_extracts_text_blocks_when_raw_output_is_absent() {
        let content = vec![agent_client_protocol::v1::schema::ToolCallContent::from(
            ContentBlock::from("file contents"),
        )];
        assert_eq!(
            tool_output(None, content),
            serde_json::Value::String("file contents".into())
        );
    }

    #[test]
    fn thought_chunks_and_mcp_raw_outputs_are_extracted() {
        let thought = notification_event(SessionUpdate::AgentThoughtChunk(
            agent_client_protocol::v1::schema::ContentChunk::new("thinking step".into()),
        ));
        assert!(matches!(
            thought,
            Some(HarnessEvent::ThoughtChunk(ref text)) if text == "thinking step"
        ));
        let mcp_raw = serde_json::json!({
            "content": [{"type": "text", "text": "line 1"}, {"type": "text", "text": "line 2"}]
        });
        assert_eq!(
            tool_output(Some(mcp_raw), Vec::new()),
            serde_json::Value::String("line 1\nline 2".into())
        );
    }
}
