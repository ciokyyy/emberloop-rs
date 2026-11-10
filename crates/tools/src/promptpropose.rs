//! `prompt_propose`: stage a prompt edit for the User. It never changes the active prompt.

use prompts::{PromptStore, ProposalStore};
use serde::Deserialize;
use serde_json::Value;

use crate::{ToolError, ToolResult, Toolbox};

#[derive(Deserialize)]
struct Args {
    id: String,
    text: Option<String>,
    reason: Option<String>,
}

/// `id` alone returns the active text, which the review cannot `read`: it sits outside the Project.
pub fn run(toolbox: &Toolbox, args: &Value) -> ToolResult<String> {
    let Args { id, text, reason } = serde_json::from_value(args.clone())?;
    let dir = toolbox
        .prompts_dir()
        .ok_or_else(|| ToolError::msg("prompt_propose needs the daemon's prompts directory"))?;
    let prompts = PromptStore::new(dir);
    let (text, reason) = match (text, reason) {
        (None, None) => {
            return prompts
                .text(&id)
                .ok_or_else(|| ToolError::msg(format!("prompt not found: {id}")));
        }
        (Some(text), Some(reason)) => (text, reason),
        _ => {
            return Err(ToolError::msg(
                "give both text and reason, or only id to read the active text",
            ))
        }
    };
    ProposalStore::for_prompts_dir(dir)
        .propose(&prompts, &id, &text, &reason)
        .map_err(|error| ToolError::msg(error.to_string()))?;
    Ok(format!(
        "Proposal for {id} staged. Nothing changes until the User approves it in Settings; an approved text reaches new Sessions only.\n"
    ))
}
