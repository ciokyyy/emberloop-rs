//! Prompt edits staged for the User. The background review cannot change a prompt; it writes a
//! proposal here, and only the User's approval turns it into an override. One pending proposal
//! per prompt id, as `<id>.json` in `<config root>/prompt-proposals`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{hash_text, validate, PromptError, PromptStore, PROMPT_IDS};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Proposal {
    pub id: String,
    pub text: String,
    pub reason: String,
    /// Hash of the active text when the proposal was written.
    pub base_hash: String,
}

impl Proposal {
    /// The active text changed after the proposal was written.
    pub fn is_stale(&self, prompts: &PromptStore) -> bool {
        prompts.active_hash(&self.id).as_deref() != Some(self.base_hash.as_str())
    }
}

#[derive(Debug, Clone)]
pub struct ProposalStore {
    dir: PathBuf,
}

impl ProposalStore {
    /// The store beside a prompts directory: `<config root>/prompts` -> `<config root>/prompt-proposals`.
    pub fn for_prompts_dir(prompts_dir: &Path) -> Self {
        Self {
            dir: prompts_dir.with_file_name("prompt-proposals"),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Stages `text` for prompt `id`, replacing any pending proposal for it.
    pub fn propose(
        &self,
        prompts: &PromptStore,
        id: &str,
        text: &str,
        reason: &str,
    ) -> Result<Proposal, PromptError> {
        let active = known(id).and_then(|()| {
            prompts
                .text(id)
                .ok_or_else(|| PromptError::NotFound { id: id.to_string() })
        })?;
        let invalid = |message: &str| PromptError::Invalid {
            id: id.to_string(),
            message: message.to_string(),
        };
        if text.trim().is_empty() {
            return Err(invalid("the proposed text is empty"));
        }
        if reason.trim().is_empty() {
            return Err(invalid("a reason is required"));
        }
        if text == active {
            return Err(invalid("the proposed text equals the active text"));
        }
        validate(id, text)?;
        let proposal = Proposal {
            id: id.to_string(),
            text: text.to_string(),
            reason: reason.to_string(),
            base_hash: hash_text(&active),
        };
        let json = serde_json::to_string_pretty(&proposal).map_err(|err| io(id, &err))?;
        std::fs::create_dir_all(&self.dir).map_err(|err| io(id, &err))?;
        // One rename, so a reader never sees a half-written file.
        let staged = self.dir.join(format!(".{id}.json.staged"));
        std::fs::write(&staged, json).map_err(|err| io(id, &err))?;
        std::fs::rename(&staged, self.path(id)).map_err(|err| io(id, &err))?;
        Ok(proposal)
    }

    pub fn get(&self, id: &str) -> Result<Proposal, PromptError> {
        let missing = || PromptError::ProposalNotFound { id: id.to_string() };
        known(id).map_err(|_| missing())?;
        let raw = std::fs::read_to_string(self.path(id)).map_err(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                missing()
            } else {
                io(id, &err)
            }
        })?;
        serde_json::from_str(&raw).map_err(|err| io(id, &err))
    }

    /// Pending proposals by id. An unreadable file is skipped, not fatal.
    pub fn list(&self) -> Vec<Proposal> {
        PROMPT_IDS
            .iter()
            .filter_map(|id| self.get(id).ok())
            .collect()
    }

    pub fn discard(&self, id: &str) -> Result<(), PromptError> {
        self.get(id)?;
        std::fs::remove_file(self.path(id)).map_err(|err| io(id, &err))
    }

    /// Writes the proposal as the override, then drops it. A stale proposal still applies:
    /// the User saw both texts.
    pub fn approve(&self, prompts: &PromptStore, id: &str) -> Result<(), PromptError> {
        let proposal = self.get(id)?;
        prompts.set_override(id, &proposal.text)?;
        self.discard(id)
    }

    fn path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }
}

/// Only known ids reach the filesystem, so an id never escapes the directory.
fn known(id: &str) -> Result<(), PromptError> {
    PROMPT_IDS
        .contains(&id)
        .then_some(())
        .ok_or_else(|| PromptError::NotFound { id: id.to_string() })
}

fn io(id: &str, err: &dyn std::fmt::Display) -> PromptError {
    PromptError::Io {
        id: id.to_string(),
        message: err.to_string(),
    }
}
