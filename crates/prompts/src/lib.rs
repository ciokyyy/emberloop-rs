#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
//! Prompts as files.
//!
//! Every model-facing text the daemon owns has a stable id, an embedded default
//! (`include_str!`) and an optional override in `<config root>/prompts/<id>.md`.
//! A Session pins the text it renders when it starts, so a reload never changes
//! a running Session's prompt (or the hashes recorded in its metadata).

pub mod defaults;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde::Serialize;
use sha2::{Digest, Sha256};

pub const PONYTAIL: &str = "ponytail";
pub const CAVEMAN: &str = "caveman";
pub const POLICY: &str = "policy";
/// The `execute`-only code graph skill, injected when the graph server is up.
pub const SKILL_CODEBASE_MEMORY: &str = "skill.codebase-memory";
pub const COMPRESS: &str = "compress";
pub const DECOMPRESS: &str = "decompress";
pub const SEARCH_CONTEXT: &str = "search_context";
pub const ACP_STATUS_DESCRIPTION: &str = "acp_status.description";
pub const COMPRESS_PROMPT: &str = "compress.prompt";
pub const TOOL_READ: &str = "tool.read.description";
pub const TOOL_EDIT: &str = "tool.edit.description";
pub const TOOL_BASH: &str = "tool.bash.description";
pub const TOOL_BACKGROUND: &str = "tool.background.description";
pub const TOOL_GREP: &str = "tool.grep.description";
pub const TOOL_WRITE: &str = "tool.write.description";
pub const TOOL_FIND: &str = "tool.find.description";
pub const TOOL_WEBSEARCH: &str = "tool.websearch.description";
pub const TOOL_EXECUTE: &str = "tool.execute.description";
pub const TOOL_TODO: &str = "tool.todo.description";
pub const TOOL_GIT: &str = "tool.git.description";
pub const TOOL_CHECK: &str = "tool.check.description";
pub const TOOL_CTX_SEARCH: &str = "tool.ctx_search.description";
/// Billion-context nudges (ticket 20). Derived from the reference wording; the
/// emergency id shares the pressure body. Raw templates: see `PinnedPrompts::pin`.
pub const NUDGE_FIRST: &str = "nudge.first";
pub const NUDGE_PRESSURE: &str = "nudge.pressure";
pub const NUDGE_EMERGENCY: &str = "nudge.emergency";
pub const NUDGE_TIER2: &str = "nudge.tier2";
pub const NUDGE_TIER3: &str = "nudge.tier3";

pub const PROMPT_IDS: &[&str] = &[
    POLICY,
    COMPRESS,
    PONYTAIL,
    CAVEMAN,
    DECOMPRESS,
    SEARCH_CONTEXT,
    ACP_STATUS_DESCRIPTION,
    COMPRESS_PROMPT,
    TOOL_READ,
    TOOL_EDIT,
    TOOL_BASH,
    TOOL_BACKGROUND,
    TOOL_GREP,
    TOOL_WRITE,
    TOOL_FIND,
    TOOL_WEBSEARCH,
    TOOL_EXECUTE,
    TOOL_TODO,
    TOOL_GIT,
    TOOL_CHECK,
    TOOL_CTX_SEARCH,
    SKILL_CODEBASE_MEMORY,
    NUDGE_FIRST,
    NUDGE_PRESSURE,
    NUDGE_EMERGENCY,
    NUDGE_TIER2,
    NUDGE_TIER3,
];

pub const ALLOWED_PLACEHOLDERS: &[&str] = &[
    "project_name",
    "project_root",
    "level",
    "date",
    "session_id",
    "context_breakdown",
    "since_last_nudge",
    "compressible_ranges",
];

/// The tool a `tool.<name>.description` id describes.
pub fn tool_name(id: &str) -> Option<&str> {
    id.strip_prefix("tool.")?.strip_suffix(".description")
}

pub fn default_text(id: &str) -> &'static str {
    match id {
        PONYTAIL => defaults::PONYTAIL,
        CAVEMAN => defaults::CAVEMAN,
        POLICY => defaults::POLICY,
        COMPRESS => defaults::COMPRESS,
        DECOMPRESS => defaults::DECOMPRESS,
        SEARCH_CONTEXT => defaults::SEARCH_CONTEXT,
        ACP_STATUS_DESCRIPTION => defaults::ACP_STATUS_DESCRIPTION,
        COMPRESS_PROMPT => defaults::COMPRESS_PROMPT,
        TOOL_READ => defaults::TOOL_READ,
        TOOL_EDIT => defaults::TOOL_EDIT,
        TOOL_BASH => defaults::TOOL_BASH,
        TOOL_BACKGROUND => defaults::TOOL_BACKGROUND,
        TOOL_GREP => defaults::TOOL_GREP,
        TOOL_WRITE => defaults::TOOL_WRITE,
        TOOL_FIND => defaults::TOOL_FIND,
        TOOL_WEBSEARCH => defaults::TOOL_WEBSEARCH,
        TOOL_EXECUTE => defaults::TOOL_EXECUTE,
        TOOL_TODO => defaults::TOOL_TODO,
        TOOL_GIT => defaults::TOOL_GIT,
        TOOL_CHECK => defaults::TOOL_CHECK,
        TOOL_CTX_SEARCH => defaults::TOOL_CTX_SEARCH,
        SKILL_CODEBASE_MEMORY => defaults::SKILL_CODEBASE_MEMORY,
        NUDGE_FIRST => defaults::NUDGE_FIRST,
        NUDGE_PRESSURE | NUDGE_EMERGENCY => defaults::NUDGE_PRESSURE,
        NUDGE_TIER2 => defaults::NUDGE_TIER2,
        NUDGE_TIER3 => defaults::NUDGE_TIER3,
        _ => "",
    }
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum PromptError {
    #[error("unknown placeholder {{{name}}} in prompt {id}")]
    UnknownPlaceholder { id: String, name: String },
    #[error("unterminated placeholder in prompt {id}")]
    UnterminatedPlaceholder { id: String },
    #[error("IO error reading prompt {id}: {message}")]
    Io { id: String, message: String },
    #[error("prompt not found: {id}")]
    NotFound { id: String },
}

impl PromptError {
    pub fn id(&self) -> &str {
        match self {
            Self::UnknownPlaceholder { id, .. }
            | Self::UnterminatedPlaceholder { id }
            | Self::Io { id, .. }
            | Self::NotFound { id } => id,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PromptInfo {
    pub id: String,
    pub default_hash: String,
    pub override_hash: Option<String>,
    pub active_hash: String,
    pub differs: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct PromptVars {
    pub project_name: String,
    pub project_root: String,
    pub level: String,
    pub date: String,
    pub session_id: String,
    /// Nudge-only: the `Context breakdown:` line, label included.
    pub context_breakdown: String,
    /// Nudge-only: the `+X since last nudge` line.
    pub since_last_nudge: String,
    /// Nudge-only: the compressible range block, header included.
    pub compressible_ranges: String,
}

impl PromptVars {
    pub fn get(&self, name: &str) -> Option<&str> {
        match name {
            "project_name" => Some(&self.project_name),
            "project_root" => Some(&self.project_root),
            "level" => Some(&self.level),
            "date" => Some(&self.date),
            "session_id" => Some(&self.session_id),
            "context_breakdown" => Some(&self.context_breakdown),
            "since_last_nudge" => Some(&self.since_last_nudge),
            "compressible_ranges" => Some(&self.compressible_ranges),
            _ => None,
        }
    }
}

#[derive(Default)]
struct Inner {
    texts: HashMap<String, String>,
    overrides: HashMap<String, String>,
    errors: Vec<PromptError>,
}

pub struct PromptStore {
    dir: PathBuf,
    inner: RwLock<Inner>,
}

impl PromptStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        let store = Self {
            dir: dir.into(),
            inner: RwLock::new(Inner::default()),
        };
        let _ = store.reload();
        store
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn override_path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.md"))
    }

    /// Re-read every override. A broken override keeps the last good text and
    /// returns its error; a missing or empty one restores the embedded default.
    pub fn reload(&self) -> Vec<PromptError> {
        let current = self
            .inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .texts
            .clone();
        let mut texts = HashMap::new();
        let mut overrides = HashMap::new();
        let mut errors = Vec::new();

        for id in PROMPT_IDS {
            let fallback = current
                .get(*id)
                .cloned()
                .unwrap_or_else(|| default_text(id).to_string());
            match read_override(&self.override_path(id), id) {
                Ok(None) => {
                    texts.insert((*id).to_string(), default_text(id).to_string());
                }
                Ok(Some(raw)) => match validate(id, &raw) {
                    Ok(()) => {
                        texts.insert((*id).to_string(), raw.clone());
                        overrides.insert((*id).to_string(), raw);
                    }
                    Err(err) => {
                        errors.push(err);
                        texts.insert((*id).to_string(), fallback);
                    }
                },
                Err(err) => {
                    errors.push(err);
                    texts.insert((*id).to_string(), fallback);
                }
            }
        }

        let mut inner = self
            .inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *inner = Inner {
            texts,
            overrides,
            errors: errors.clone(),
        };
        errors
    }

    pub fn errors(&self) -> Vec<PromptError> {
        self.inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .errors
            .clone()
    }

    pub fn text(&self, id: &str) -> Option<String> {
        self.inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .texts
            .get(id)
            .cloned()
    }

    pub fn raw_override(&self, id: &str) -> Option<String> {
        self.inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .overrides
            .get(id)
            .cloned()
    }

    pub fn has_override(&self, id: &str) -> bool {
        self.inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .overrides
            .contains_key(id)
    }
    pub fn render(&self, id: &str, vars: &PromptVars) -> Result<String, PromptError> {
        let text = self
            .text(id)
            .ok_or_else(|| PromptError::NotFound { id: id.to_string() })?;
        substitute(id, &text, vars)
    }

    pub fn active_hash(&self, id: &str) -> Option<String> {
        self.text(id).map(|text| hash_text(&text))
    }

    pub fn list(&self) -> Vec<PromptInfo> {
        let inner = self
            .inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        PROMPT_IDS
            .iter()
            .map(|id| {
                let active = inner.texts.get(*id).cloned().unwrap_or_default();
                let raw = inner.overrides.get(*id);
                PromptInfo {
                    id: (*id).to_string(),
                    default_hash: hash_text(default_text(id)),
                    override_hash: raw.map(|text| hash_text(text)),
                    active_hash: hash_text(&active),
                    differs: raw.is_some(),
                    error: inner
                        .errors
                        .iter()
                        .find(|err| err.id() == *id)
                        .map(ToString::to_string),
                }
            })
            .collect()
    }

    pub fn set_override(&self, id: &str, text: &str) -> Result<(), PromptError> {
        if !PROMPT_IDS.contains(&id) {
            return Err(PromptError::NotFound { id: id.to_string() });
        }
        if text.trim().is_empty() {
            return self.remove_override(id);
        }
        validate(id, text)?;
        std::fs::create_dir_all(&self.dir).map_err(|err| PromptError::Io {
            id: id.to_string(),
            message: err.to_string(),
        })?;
        std::fs::write(self.override_path(id), text).map_err(|err| PromptError::Io {
            id: id.to_string(),
            message: err.to_string(),
        })?;

        let mut inner = self
            .inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.texts.insert(id.to_string(), text.to_string());
        inner.overrides.insert(id.to_string(), text.to_string());
        inner.errors.retain(|err| err.id() != id);
        Ok(())
    }

    pub fn remove_override(&self, id: &str) -> Result<(), PromptError> {
        if !PROMPT_IDS.contains(&id) {
            return Err(PromptError::NotFound { id: id.to_string() });
        }
        match std::fs::remove_file(self.override_path(id)) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                return Err(PromptError::Io {
                    id: id.to_string(),
                    message: err.to_string(),
                })
            }
        }
        let mut inner = self
            .inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner
            .texts
            .insert(id.to_string(), default_text(id).to_string());
        inner.overrides.remove(id);
        inner.errors.retain(|err| err.id() != id);
        Ok(())
    }
}

fn read_override(path: &Path, id: &str) -> Result<Option<String>, PromptError> {
    match std::fs::read_to_string(path) {
        Ok(raw) if raw.trim().is_empty() => Ok(None),
        Ok(raw) => Ok(Some(raw)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(PromptError::Io {
            id: id.to_string(),
            message: err.to_string(),
        }),
    }
}

fn validate(id: &str, text: &str) -> Result<(), PromptError> {
    placeholders(id, text).map(|_| ())
}

fn placeholders(id: &str, text: &str) -> Result<Vec<String>, PromptError> {
    let mut names = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            return Err(PromptError::UnterminatedPlaceholder { id: id.to_string() });
        };
        let name = after[..end].trim();
        let valid_name =
            !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !valid_name || !ALLOWED_PLACEHOLDERS.contains(&name) {
            return Err(PromptError::UnknownPlaceholder {
                id: id.to_string(),
                name: name.to_string(),
            });
        }
        names.push(name.to_string());
        rest = &after[end + 2..];
    }
    Ok(names)
}

/// Renders a raw template (from `PinnedPrompts::text` of a nudge id) with per-request vars.
pub fn render_text(id: &str, text: &str, vars: &PromptVars) -> Result<String, PromptError> {
    substitute(id, text, vars)
}

fn substitute(id: &str, text: &str, vars: &PromptVars) -> Result<String, PromptError> {
    placeholders(id, text)?;
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            break;
        };
        let name = after[..end].trim();
        out.push_str(vars.get(name).unwrap_or(""));
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

pub fn hash_text(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    hex::encode(digest)
}

pub fn normalize_ponytail_level(level: &str) -> &'static str {
    match level.trim().to_ascii_lowercase().as_str() {
        "off" => "off",
        "lite" => "lite",
        "ultra" => "ultra",
        _ => "full",
    }
}

/// The Ponytail text for a level: `off` is empty; every other level keeps its
/// own row and drops the rest of the intensity table.
pub fn ponytail_text(
    store: &PromptStore,
    level: &str,
    vars: &PromptVars,
) -> Result<String, PromptError> {
    let level = normalize_ponytail_level(level);
    if level == "off" {
        return Ok(String::new());
    }
    let text = store.render(PONYTAIL, vars)?;
    Ok(filter_skill(&text, level))
}

/// The always-ultra caveman register: the plugin's banner, its ultra ruleset and
/// its one-line reminder. Copied from emberloop-go's `el/voice.go`.
pub fn caveman_text(store: &PromptStore, vars: &PromptVars) -> Result<String, PromptError> {
    let skill = filter_skill(&store.render(CAVEMAN, vars)?, "ultra");
    Ok(format!("{CAVEMAN_BANNER}\n\n{skill}\n\n{CAVEMAN_REMINDER}"))
}

const CAVEMAN_BANNER: &str = "CAVEMAN MODE ACTIVE — level: ultra";

const CAVEMAN_REMINDER: &str = "CAVEMAN MODE ACTIVE (ultra). Enforce this reply: \
     Drop articles, filler, pleasantries, hedging, and excess conjunctions. Prefer \
     fragments over full natural-prose sentences. State each fact once. No preamble or \
     recap. Technical terms, code, commands, paths, and errors stay exact.";

/// A skill body for one level: frontmatter stripped, only that level's row of the
/// intensity table and its example lines kept.
fn filter_skill(skill: &str, level: &str) -> String {
    strip_frontmatter(skill)
        .lines()
        .filter(|line| keep_skill_line(line, level))
        .collect::<Vec<_>>()
        .join("\n")
}

fn strip_frontmatter(text: &str) -> &str {
    let Some(rest) = text.strip_prefix("---") else {
        return text;
    };
    let Some(end) = rest.find("\n---") else {
        return text;
    };
    rest[end + 4..].trim_start_matches(['\n', '\r'])
}

fn keep_skill_line(line: &str, level: &str) -> bool {
    let trimmed = line.trim_start();
    if trimmed.starts_with("| **") {
        return trimmed
            .split("**")
            .nth(1)
            .is_some_and(|name| name.trim() == level);
    }
    // A `- lite: ...` example names its level in one token; prose bullets that
    // happen to hold a colon are not examples.
    if let Some(after) = trimmed.strip_prefix("- ") {
        if let Some((name, _)) = after.split_once(':') {
            if !name.is_empty() && !name.contains(char::is_whitespace) {
                return name == level;
            }
        }
    }
    true
}

/// One Session's pinned prompt texts and their content hashes.
#[derive(Debug, Clone, Default)]
pub struct PinnedPrompts {
    texts: HashMap<String, String>,
    hashes: HashMap<String, String>,
}

impl PinnedPrompts {
    pub fn pin(store: &PromptStore, vars: &PromptVars) -> Result<Self, PromptError> {
        let mut pins = Self::default();
        for id in PROMPT_IDS {
            let text = match *id {
                PONYTAIL => ponytail_text(store, &vars.level, vars)?,
                CAVEMAN => caveman_text(store, vars)?,
                NUDGE_FIRST | NUDGE_PRESSURE | NUDGE_EMERGENCY | NUDGE_TIER2 | NUDGE_TIER3 => {
                    let raw = store.text(id).unwrap_or_default();
                    validate(id, &raw)?;
                    raw
                }
                _ => store.render(id, vars)?,
            };
            pins.hashes.insert((*id).to_string(), hash_text(&text));
            pins.texts.insert((*id).to_string(), text);
        }
        Ok(pins)
    }

    pub fn text(&self, id: &str) -> Option<&str> {
        self.texts.get(id).map(String::as_str)
    }

    pub fn hash(&self, id: &str) -> Option<&str> {
        self.hashes.get(id).map(String::as_str)
    }

    pub fn hashes(&self) -> HashMap<String, String> {
        self.hashes.clone()
    }
}

pub struct PromptWatcher {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl PromptWatcher {
    pub fn stop(mut self) {
        self.halt();
    }

    fn halt(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for PromptWatcher {
    fn drop(&mut self) {
        self.halt();
    }
}

/// Watch the override directory by polling. A reload that fails reports its
/// error and keeps the last good text on the next request that pins a Session.
///
/// shortcut: polling every `interval`, replace with `notify` when inotify
/// events matter more than one small dependency. The real wait is the product
/// behaviour; the check bans direct sleeps everywhere else, tests included.
#[allow(clippy::disallowed_methods)]
pub fn watch<F>(store: Arc<PromptStore>, interval: Duration, on_error: F) -> PromptWatcher
where
    F: Fn(PromptError) + Send + 'static,
{
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = stop.clone();
    let handle = thread::spawn(move || {
        while !thread_stop.load(Ordering::Relaxed) {
            thread::sleep(interval);
            if thread_stop.load(Ordering::Relaxed) {
                break;
            }
            for error in store.reload() {
                on_error(error);
            }
        }
    });
    PromptWatcher {
        stop,
        thread: Some(handle),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests;
