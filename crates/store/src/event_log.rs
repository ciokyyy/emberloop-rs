use emberloop_core::events::SessionEvent;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;

const DEFAULT_SEGMENT_BYTES: u64 = 4 * 1024 * 1024;
const RING_CAPACITY: usize = 200;
const INDEX_FILE: &str = "index.json";

#[derive(Debug, thiserror::Error)]
pub enum EventLogError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Serialization error: {0}")]
    Serde(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Segment {
    name: String,
    first_seq: u64,
    bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TurnIndex {
    turn: u64,
    seq: u64,
    segment: String,
    offset: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Sidecar {
    segments: Vec<Segment>,
    turns: Vec<TurnIndex>,
}

struct LogState {
    next_seq: u64,
    turn: u64,
    segments: Vec<Segment>,
    turns: Vec<TurnIndex>,
}

#[derive(Clone)]
pub struct EventLog {
    dir: PathBuf,
    segment_bytes: u64,
    state: Arc<Mutex<LogState>>,
    ring: Arc<Mutex<VecDeque<SessionEvent>>>,
}

impl EventLog {
    pub async fn open(dir: impl Into<PathBuf>) -> Result<Self, EventLogError> {
        Self::open_with_segment_bytes(dir, DEFAULT_SEGMENT_BYTES).await
    }

    pub async fn open_with_segment_bytes(
        dir: impl Into<PathBuf>,
        segment_bytes: u64,
    ) -> Result<Self, EventLogError> {
        let dir = dir.into();
        tokio::fs::create_dir_all(&dir).await?;
        let mut sidecar = load_sidecar(&dir).await.unwrap_or_default();
        if sidecar.segments.is_empty() {
            sidecar.segments = scan_segments(&dir).await?;
        }
        sidecar
            .segments
            .retain(|segment| dir.join(&segment.name).exists());
        let (max_seq, last_bytes) = last_segment_state(&dir, &sidecar.segments).await?;
        if let Some(last) = sidecar.segments.last_mut() {
            last.bytes = last_bytes;
        }
        let turn = sidecar.turns.last().map(|turn| turn.turn).unwrap_or(0);
        Ok(Self {
            dir,
            segment_bytes,
            state: Arc::new(Mutex::new(LogState {
                next_seq: max_seq + 1,
                turn,
                segments: sidecar.segments,
                turns: sidecar.turns,
            })),
            ring: Arc::new(Mutex::new(VecDeque::with_capacity(RING_CAPACITY))),
        })
    }

    pub async fn append(&self, mut event: SessionEvent) -> Result<SessionEvent, EventLogError> {
        let mut state = self.state.lock().await;
        event.seq = state.next_seq;
        state.next_seq += 1;

        let mut line = serde_json::to_vec(&event)?;
        line.push(b'\n');
        let rotated = state.segments.is_empty()
            || (state
                .segments
                .last()
                .map(|segment| segment.bytes)
                .unwrap_or(0)
                > 0
                && state
                    .segments
                    .last()
                    .map(|segment| segment.bytes + line.len() as u64 > self.segment_bytes)
                    .unwrap_or(false));
        if rotated {
            let name = format!("{:06}.jsonl", state.segments.len());
            state.segments.push(Segment {
                name,
                first_seq: event.seq,
                bytes: 0,
            });
        }
        let Some(segment_name) = state.segments.last().map(|segment| segment.name.clone()) else {
            return Err(EventLogError::Io(std::io::Error::other(
                "event log has no segment",
            )));
        };
        let path = self.dir.join(&segment_name);
        let offset = state
            .segments
            .last()
            .map(|segment| segment.bytes)
            .unwrap_or(0);
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await?;
        file.write_all(&line).await?;
        file.flush().await?;
        if let Some(segment) = state.segments.last_mut() {
            segment.bytes += line.len() as u64;
        }

        let is_prompt = matches!(event.kind, emberloop_core::events::EventKind::Prompt);
        if is_prompt {
            state.turn += 1;
            let turn = state.turn;
            state.turns.push(TurnIndex {
                turn,
                seq: event.seq,
                segment: segment_name,
                offset,
            });
        }
        if rotated || is_prompt {
            write_sidecar(&self.dir, &state.segments, &state.turns).await?;
        }
        drop(state);

        let mut ring = self.ring.lock().await;
        if ring.len() == RING_CAPACITY {
            ring.pop_front();
        }
        ring.push_back(event.clone());
        drop(ring);
        Ok(event)
    }

    pub async fn replay(&self, after: Option<u64>) -> Result<Vec<SessionEvent>, EventLogError> {
        let segments = {
            let state = self.state.lock().await;
            state.segments.clone()
        };
        let mut events = Vec::new();
        for segment in segments {
            let file = tokio::fs::File::open(self.dir.join(&segment.name)).await?;
            let mut lines = BufReader::new(file).lines();
            while let Some(line) = lines.next_line().await? {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let Ok(event) = serde_json::from_str::<SessionEvent>(trimmed) else {
                    continue;
                };
                if after.map(|seq| event.seq > seq).unwrap_or(true) {
                    events.push(event);
                }
            }
        }
        Ok(events)
    }

    /// Reads only as many newest segments as the window needs.
    pub async fn tail(&self, limit: usize) -> Result<Vec<SessionEvent>, EventLogError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let mut names = {
            let state = self.state.lock().await;
            state
                .segments
                .iter()
                .map(|segment| segment.name.clone())
                .collect::<Vec<_>>()
        };
        let mut events = VecDeque::with_capacity(limit);
        while let Some(name) = names.pop() {
            let text = tokio::fs::read_to_string(self.dir.join(name)).await?;
            let mut segment_events = text
                .lines()
                .filter(|line| !line.trim().is_empty())
                .filter_map(|line| serde_json::from_str::<SessionEvent>(line).ok())
                .collect::<VecDeque<_>>();
            while let Some(event) = segment_events.pop_back() {
                events.push_front(event);
                if events.len() == limit {
                    return Ok(events.into_iter().collect());
                }
            }
        }
        Ok(events.into_iter().collect())
    }

    pub async fn recent(&self, limit: usize) -> Vec<SessionEvent> {
        let ring = self.ring.lock().await;
        let start = ring.len().saturating_sub(limit);
        ring.iter().skip(start).cloned().collect()
    }

    pub async fn last_seq(&self) -> u64 {
        self.state.lock().await.next_seq.saturating_sub(1)
    }

    pub async fn turn_index(&self) -> Vec<(u64, String, u64)> {
        self.state
            .lock()
            .await
            .turns
            .iter()
            .map(|turn| (turn.turn, turn.segment.clone(), turn.offset))
            .collect()
    }
}

async fn load_sidecar(dir: &Path) -> Result<Sidecar, EventLogError> {
    let Ok(text) = tokio::fs::read_to_string(dir.join(INDEX_FILE)).await else {
        return Ok(Sidecar::default());
    };
    Ok(serde_json::from_str(&text).unwrap_or_default())
}

async fn write_sidecar(
    dir: &Path,
    segments: &[Segment],
    turns: &[TurnIndex],
) -> Result<(), EventLogError> {
    let sidecar = Sidecar {
        segments: segments.to_vec(),
        turns: turns.to_vec(),
    };
    let path = dir.join(INDEX_FILE);
    let tmp = dir.join(format!("{INDEX_FILE}.tmp.{}", std::process::id()));
    tokio::fs::write(&tmp, serde_json::to_vec(&sidecar)?).await?;
    tokio::fs::rename(tmp, path).await?;
    Ok(())
}

async fn scan_segments(dir: &Path) -> Result<Vec<Segment>, EventLogError> {
    let mut files = Vec::new();
    let mut entries = tokio::fs::read_dir(dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.ends_with(".jsonl") {
            files.push(name);
        }
    }
    files.sort();
    let mut segments = Vec::new();
    for name in files {
        let first_seq = first_seq(&dir.join(&name)).await?;
        let bytes = tokio::fs::metadata(dir.join(&name)).await?.len();
        segments.push(Segment {
            name,
            first_seq,
            bytes,
        });
    }
    Ok(segments)
}

async fn first_seq(path: &Path) -> Result<u64, EventLogError> {
    let Ok(file) = tokio::fs::File::open(path).await else {
        return Ok(0);
    };
    let mut lines = BufReader::new(file).lines();
    while let Some(line) = lines.next_line().await? {
        if let Ok(event) = serde_json::from_str::<SessionEvent>(line.trim()) {
            return Ok(event.seq);
        }
    }
    Ok(0)
}

async fn last_segment_state(dir: &Path, segments: &[Segment]) -> Result<(u64, u64), EventLogError> {
    let Some(segment) = segments.last() else {
        return Ok((0, 0));
    };
    let path = dir.join(&segment.name);
    let bytes = tokio::fs::metadata(&path)
        .await
        .map(|meta| meta.len())
        .unwrap_or(0);
    let file = tokio::fs::File::open(&path).await?;
    let mut lines = BufReader::new(file).lines();
    let mut max_seq = 0;
    while let Some(line) = lines.next_line().await? {
        if let Ok(event) = serde_json::from_str::<SessionEvent>(line.trim()) {
            max_seq = max_seq.max(event.seq);
        }
    }
    Ok((max_seq, bytes))
}
