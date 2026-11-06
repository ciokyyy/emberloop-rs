//! What counts as one block of a Session feed, so a window of "the last N blocks" starts where a
//! reader expects. A run of assistant or thinking chunks is one block, a tool call is one block
//! however many of its events interleave with others, and a prompt, notice, error or steer is a
//! block of its own. Status, usage and the like ride along with the block they sit in.

use std::collections::HashSet;

use emberloop_core::events::{EventKind, SessionEvent};

enum Key {
    Chunk(bool),
    Tool(String),
    Own,
}

impl Key {
    fn of(event: &SessionEvent) -> Option<Self> {
        match &event.kind {
            EventKind::AssistantChunk => Some(Self::Chunk(false)),
            EventKind::ThinkingChunk => Some(Self::Chunk(true)),
            EventKind::ToolStarted | EventKind::ToolFinished => Some(
                event
                    .data
                    .get("id")
                    .and_then(|id| id.as_str())
                    .map_or(Self::Own, |id| Self::Tool(id.to_string())),
            ),
            EventKind::Prompt
            | EventKind::Notice
            | EventKind::Error
            | EventKind::SteerQueued
            | EventKind::SteerDelivered => Some(Self::Own),
            _ => None,
        }
    }
}

/// BlockWindow admits events newest first until `limit` blocks are in hand. A tool whose finish is
/// inside keeps pulling older events until its start is in too, so the window never shows a result
/// without the call that made it.
pub(crate) struct BlockWindow {
    limit: usize,
    blocks: usize,
    last_chunk: Option<bool>,
    seen_tools: HashSet<String>,
    open_tools: HashSet<String>,
}

impl BlockWindow {
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            limit,
            blocks: 0,
            last_chunk: None,
            seen_tools: HashSet::new(),
            open_tools: HashSet::new(),
        }
    }

    /// admit says whether `event` (older than every event admitted so far) belongs in the window.
    pub(crate) fn admit(&mut self, event: &SessionEvent) -> bool {
        let Some(key) = Key::of(event) else {
            return true;
        };
        if !self.continues(&key) && !self.open_block() {
            return false;
        }
        self.note(key, &event.kind);
        true
    }

    fn continues(&self, key: &Key) -> bool {
        match key {
            Key::Chunk(thinking) => self.last_chunk == Some(*thinking),
            Key::Tool(id) => self.seen_tools.contains(id),
            Key::Own => false,
        }
    }

    /// open_block counts a new block, or lets one through that only completes an open tool.
    fn open_block(&mut self) -> bool {
        if self.blocks < self.limit {
            self.blocks += 1;
            return true;
        }
        !self.open_tools.is_empty()
    }

    fn note(&mut self, key: Key, kind: &EventKind) {
        self.last_chunk = match key {
            Key::Chunk(thinking) => Some(thinking),
            Key::Tool(_) | Key::Own => None,
        };
        let Key::Tool(id) = key else {
            return;
        };
        if matches!(kind, EventKind::ToolStarted) {
            self.open_tools.remove(&id);
            self.seen_tools.insert(id);
        } else if self.seen_tools.insert(id.clone()) {
            self.open_tools.insert(id);
        }
    }
}
