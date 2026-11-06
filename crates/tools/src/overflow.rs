use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::tag::lines_of;

const HEAD_LINES: usize = 10;
const TAIL_LINES: usize = 10;
const MAX_HANDLE_LEN: usize = 128;

/// Stores oversized tool output under `<data root>/sessions/<id>/spill/`.
#[derive(Debug)]
pub struct OverflowStore {
    dir: PathBuf,
    threshold: usize,
    counter: AtomicU64,
}

impl OverflowStore {
    pub fn new(dir: impl Into<PathBuf>, threshold: usize) -> Self {
        Self {
            dir: dir.into(),
            threshold,
            counter: AtomicU64::new(0),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn maybe_spill(&self, content: String, origin: &str) -> io::Result<String> {
        if content.len() <= self.threshold {
            return Ok(content);
        }
        let handle = self.write(&content)?;
        Ok(summary(&handle, &content, origin))
    }

    pub fn write(&self, content: &str) -> io::Result<String> {
        fs::create_dir_all(&self.dir)?;
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let seq = self.counter.fetch_add(1, Ordering::Relaxed);
        let handle = format!("{millis:x}{seq:04x}");
        fs::write(self.dir.join(&handle), content)?;
        Ok(handle)
    }

    /// Resolves a ctx handle to its spill file. Handles never leave the spill dir. The proxy
    /// mints `c`-prefixed handles beside the tools' bare ones and models drop or add the `c`,
    /// so the other spelling is tried when the exact one is missing.
    pub fn path(&self, handle: &str) -> Option<PathBuf> {
        let alternate = match handle.strip_prefix('c') {
            Some(bare) => bare.to_string(),
            None => format!("c{handle}"),
        };
        self.exact(handle).or_else(|| self.exact(&alternate))
    }

    fn exact(&self, handle: &str) -> Option<PathBuf> {
        if handle.is_empty() || handle.len() > MAX_HANDLE_LEN {
            return None;
        }
        if !handle
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return None;
        }
        let path = self.dir.join(handle);
        path.is_file().then_some(path)
    }
}

fn summary(handle: &str, content: &str, origin: &str) -> String {
    let lines = lines_of(content);
    let mut out = format!(
        "[overflow] output stored as ctx:{handle} ({} bytes, {} lines) from {origin}\n",
        content.len(),
        lines.len()
    );
    push_window(&mut out, "head", &lines, 0..lines.len().min(HEAD_LINES));
    if lines.len() > HEAD_LINES + TAIL_LINES {
        push_window(
            &mut out,
            "tail",
            &lines,
            lines.len() - TAIL_LINES..lines.len(),
        );
    } else if lines.len() > HEAD_LINES {
        push_window(&mut out, "tail", &lines, HEAD_LINES..lines.len());
    }
    out.push_str(&format!("page it with read ctx:{handle}:<start>-<end>\n"));
    out
}

fn push_window(out: &mut String, label: &str, lines: &[&str], range: std::ops::Range<usize>) {
    let shown = range.len();
    out.push_str(&format!("{label} ({shown} of {} lines):\n", lines.len()));
    for line in &lines[range] {
        out.push_str(line);
        out.push('\n');
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_forgives_a_dropped_or_added_c_prefix() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("c1f0"), "proxy").expect("write");
        fs::write(dir.path().join("2a0"), "tool").expect("write");
        let store = OverflowStore::new(dir.path(), 10);
        assert_eq!(store.path("1f0"), Some(dir.path().join("c1f0")));
        assert_eq!(store.path("c2a0"), Some(dir.path().join("2a0")));
        assert_eq!(store.path("zzz"), None);
    }
}
