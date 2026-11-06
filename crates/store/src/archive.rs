use crate::event_log::EventLogError;
use crate::sessions::SessionStoreError;
use crate::{DataRoot, EventLog, SessionMeta, SessionState, SessionsStore};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const MAGIC: &[u8] = b"ELOG1";
const DAY_MS: u64 = 24 * 60 * 60 * 1000;
const HOUR_MS: u64 = 60 * 60 * 1000;

#[derive(Debug, Clone)]
pub struct ArchivalConfig {
    pub archive_sweep_hours: u64,
    pub archive_after_days: u64,
    pub restore_keep_hours: u64,
}

impl Default for ArchivalConfig {
    fn default() -> Self {
        Self {
            archive_sweep_hours: 24,
            archive_after_days: 3,
            restore_keep_hours: 24,
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct SweepReport {
    pub archived: Vec<String>,
    pub resumed: Vec<String>,
    pub removed_hot: Vec<String>,
    pub bytes_compressed: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum ArchivalError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Serialization error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("Session store: {0}")]
    Session(#[from] SessionStoreError),
    #[error("Event log: {0}")]
    EventLog(#[from] EventLogError),
    #[error("archive not found for Session {0}")]
    NotFound(String),
    #[error("corrupt archive: {0}")]
    Corrupt(String),
}

#[derive(Clone)]
pub struct ArchivalSweeper {
    data_root: DataRoot,
    sessions: SessionsStore,
    config: ArchivalConfig,
}

impl ArchivalSweeper {
    pub fn new(data_root: DataRoot, sessions: SessionsStore, config: ArchivalConfig) -> Self {
        Self {
            data_root,
            sessions,
            config,
        }
    }

    /// Runs the sweep now and then every `archive_sweep_hours`, reporting each
    /// outcome to `on_sweep`. The task ends when the returned handle is aborted.
    pub fn spawn_scheduler<F>(self, on_sweep: F) -> tokio::task::JoinHandle<()>
    where
        F: Fn(Result<SweepReport, ArchivalError>) + Send + Sync + 'static,
    {
        let period = std::time::Duration::from_secs(self.config.archive_sweep_hours.max(1) * 3600);
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(period);
            loop {
                ticker.tick().await;
                on_sweep(self.sweep().await);
            }
        })
    }

    pub async fn sweep(&self) -> Result<SweepReport, ArchivalError> {
        self.sweep_at(now_ms()).await
    }

    pub async fn sweep_at(&self, now: u64) -> Result<SweepReport, ArchivalError> {
        let mut report = SweepReport::default();
        let mut metas = self.sessions.list(None).await;
        metas.sort_by(|left, right| left.id.cmp(&right.id));
        for meta in metas {
            self.sweep_one(meta, now, &mut report).await?;
        }
        Ok(report)
    }

    async fn sweep_one(
        &self,
        mut meta: SessionMeta,
        now: u64,
        report: &mut SweepReport,
    ) -> Result<(), ArchivalError> {
        match meta.state {
            SessionState::Active => {
                let events_dir = self.data_root.session_events_dir(&meta.id);
                let archive = self.data_root.session_archive_file(&meta.id);
                if archive.exists() && !events_dir.exists() {
                    meta.state = SessionState::Archived;
                    meta.archived_at = Some(now);
                    self.sessions.save(&meta).await?;
                    report.archived.push(meta.id.clone());
                    return Ok(());
                }
                let idle_since = meta.last_used_at.unwrap_or(meta.updated_at);
                let idle_limit = self.config.archive_after_days.saturating_mul(DAY_MS);
                if now.saturating_sub(idle_since) < idle_limit {
                    return Ok(());
                }
                if self.archive(&meta.id).await? {
                    meta.state = SessionState::Archived;
                    meta.archived_at = Some(now);
                    meta.restored_at = None;
                    meta.restored_seq = None;
                    self.sessions.save(&meta).await?;
                    report.archived.push(meta.id.clone());
                }
            }
            SessionState::Restored => {
                let restored_at = meta.restored_at.unwrap_or(now);
                if now.saturating_sub(restored_at)
                    < self.config.restore_keep_hours.saturating_mul(HOUR_MS)
                {
                    return Ok(());
                }
                let log = EventLog::open(self.data_root.session_events_dir(&meta.id)).await?;
                let resumed = log.last_seq().await > meta.restored_seq.unwrap_or(0);
                if resumed {
                    remove_file_if_exists(&self.data_root.session_archive_file(&meta.id)).await?;
                    meta.state = SessionState::Active;
                    meta.archived_at = None;
                    meta.restored_at = None;
                    meta.restored_seq = None;
                    meta.last_used_at = Some(now);
                    meta.updated_at = now;
                    self.sessions.save(&meta).await?;
                    report.resumed.push(meta.id.clone());
                } else {
                    remove_dir_if_exists(&self.data_root.session_events_dir(&meta.id)).await?;
                    meta.state = SessionState::Archived;
                    self.sessions.save(&meta).await?;
                    report.removed_hot.push(meta.id.clone());
                }
            }
            SessionState::Archived => {
                let events_dir = self.data_root.session_events_dir(&meta.id);
                if self.data_root.session_archive_file(&meta.id).exists() && events_dir.exists() {
                    remove_dir_if_exists(&events_dir).await?;
                    report.removed_hot.push(meta.id.clone());
                }
            }
        }
        Ok(())
    }

    async fn archive(&self, id: &str) -> Result<bool, ArchivalError> {
        let events_dir = self.data_root.session_events_dir(id);
        let files = read_event_files(&events_dir).await?;
        if files.is_empty() {
            return Ok(false);
        }
        let payload = pack(&files)?;
        let compressed = compress(&payload)?;
        if decompress(&compressed)? != payload {
            return Err(ArchivalError::Corrupt("archive verification failed".into()));
        }

        let archive = self.data_root.session_archive_file(id);
        if let Some(parent) = archive.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let tmp = PathBuf::from(format!("{}.tmp.{}", archive.display(), std::process::id()));
        tokio::fs::write(&tmp, &compressed).await?;
        tokio::fs::rename(&tmp, &archive).await?;
        tokio::fs::remove_dir_all(&events_dir).await?;
        Ok(true)
    }

    pub async fn restore_session(&self, id: &str) -> Result<(), ArchivalError> {
        self.restore_at(id, now_ms()).await
    }

    pub async fn restore_at(&self, id: &str, now: u64) -> Result<(), ArchivalError> {
        let archive = self.data_root.session_archive_file(id);
        if !archive.exists() {
            return Err(ArchivalError::NotFound(id.to_string()));
        }
        let compressed = tokio::fs::read(&archive).await?;
        let payload = decompress(&compressed)?;
        let files = unpack(&payload)?;
        let events_dir = self.data_root.session_events_dir(id);
        remove_dir_if_exists(&events_dir).await?;
        tokio::fs::create_dir_all(&events_dir).await?;
        for (name, bytes) in files {
            let path = events_dir.join(name);
            let tmp = PathBuf::from(format!("{}.tmp.{}", path.display(), std::process::id()));
            tokio::fs::write(&tmp, bytes).await?;
            tokio::fs::rename(&tmp, path).await?;
        }

        let mut meta = self
            .sessions
            .get(id)
            .await
            .ok_or_else(|| SessionStoreError::NotFound(id.to_string()))?;
        let log = EventLog::open(&events_dir).await?;
        meta.state = SessionState::Restored;
        meta.restored_at = Some(now);
        meta.restored_seq = Some(log.last_seq().await);
        meta.last_used_at = Some(now);
        self.sessions.save(&meta).await?;
        Ok(())
    }
}

async fn read_event_files(dir: &Path) -> Result<Vec<(String, Vec<u8>)>, ArchivalError> {
    let mut files = Vec::new();
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
        return Ok(files);
    };
    while let Some(entry) = entries.next_entry().await? {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.ends_with(".jsonl") || name == "index.json" {
            files.push((name, tokio::fs::read(entry.path()).await?));
        }
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(files)
}

fn pack(files: &[(String, Vec<u8>)]) -> Result<Vec<u8>, ArchivalError> {
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(files.len() as u32).to_le_bytes());
    for (name, bytes) in files {
        let name_len = u32::try_from(name.len())
            .map_err(|_| ArchivalError::Corrupt("file name is too long".into()))?;
        out.extend_from_slice(&name_len.to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
        out.extend_from_slice(bytes);
    }
    Ok(out)
}

fn unpack(bytes: &[u8]) -> Result<Vec<(String, Vec<u8>)>, ArchivalError> {
    if !bytes.starts_with(MAGIC) {
        return Err(ArchivalError::Corrupt("bad magic".into()));
    }
    let mut offset = MAGIC.len();
    let count = take_u32(bytes, &mut offset)?;
    let mut files = Vec::new();
    for _ in 0..count {
        let name_len = take_u32(bytes, &mut offset)? as usize;
        let name = take(bytes, &mut offset, name_len)?;
        let name = String::from_utf8(name.to_vec())
            .map_err(|_| ArchivalError::Corrupt("file name is not UTF-8".into()))?;
        if !safe_name(&name) {
            return Err(ArchivalError::Corrupt("unsafe archive file name".into()));
        }
        let data_len = take_u64(bytes, &mut offset)? as usize;
        let data = take(bytes, &mut offset, data_len)?.to_vec();
        files.push((name, data));
    }
    Ok(files)
}

fn safe_name(name: &str) -> bool {
    name == "index.json"
        || (name.ends_with(".jsonl")
            && !name.contains(['/', '\\'])
            && name != ".."
            && !name.is_empty())
}

fn take<'a>(bytes: &'a [u8], offset: &mut usize, len: usize) -> Result<&'a [u8], ArchivalError> {
    let end = offset
        .checked_add(len)
        .ok_or_else(|| ArchivalError::Corrupt("length overflow".into()))?;
    let slice = bytes
        .get(*offset..end)
        .ok_or_else(|| ArchivalError::Corrupt("truncated archive".into()))?;
    *offset = end;
    Ok(slice)
}

fn take_u32(bytes: &[u8], offset: &mut usize) -> Result<u32, ArchivalError> {
    let slice = take(bytes, offset, 4)?;
    let array: [u8; 4] = slice
        .try_into()
        .map_err(|_| ArchivalError::Corrupt("bad u32".into()))?;
    Ok(u32::from_le_bytes(array))
}

fn take_u64(bytes: &[u8], offset: &mut usize) -> Result<u64, ArchivalError> {
    let slice = take(bytes, offset, 8)?;
    let array: [u8; 8] = slice
        .try_into()
        .map_err(|_| ArchivalError::Corrupt("bad u64".into()))?;
    Ok(u64::from_le_bytes(array))
}

fn compress(payload: &[u8]) -> Result<Vec<u8>, ArchivalError> {
    let mut encoder = zstd::stream::Encoder::new(Vec::new(), 19)?;
    encoder.long_distance_matching(true)?;
    encoder.write_all(payload)?;
    Ok(encoder.finish()?)
}

fn decompress(compressed: &[u8]) -> Result<Vec<u8>, ArchivalError> {
    Ok(zstd::stream::decode_all(compressed)?)
}

async fn remove_file_if_exists(path: &Path) -> Result<(), ArchivalError> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

async fn remove_dir_if_exists(path: &Path) -> Result<(), ArchivalError> {
    match tokio::fs::remove_dir_all(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
