use crate::{ToolError, ToolResult};
use serde_json::Value;

pub async fn run(session_url: &str, name: &str, args: &Value) -> ToolResult<String> {
    let mut body = args
        .as_object()
        .cloned()
        .ok_or_else(|| ToolError::msg("Expected an argument object"))?;
    body.insert("action".into(), name.into());
    let response = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(3))
        .timeout(std::time::Duration::from_secs(90))
        .build()
        .map_err(|error| ToolError::msg(error.to_string()))?
        .post(format!("{session_url}/subagents"))
        .json(&body)
        .send()
        .await
        .map_err(|error| ToolError::msg(format!("Reviewer action failed: {error}")))?;
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|error| ToolError::msg(error.to_string()))?;
    if !status.is_success() {
        return Err(ToolError::msg(format!("{status}: {text}")));
    }
    Ok(text)
}
