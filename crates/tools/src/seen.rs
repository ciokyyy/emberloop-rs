use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Default, Serialize, Deserialize)]
struct Snapshot {
    #[serde(default)]
    tag: String,
    #[serde(default)]
    hash: String,
    #[serde(default)]
    lines: BTreeSet<u32>,
}

/// Lines a tool view showed, keyed by path and full snapshot hash.
///
/// The four-hex tag is only a display label. `hash` is the full sha256, so a
/// 16-bit prefix collision cannot make a stale edit look current.
pub struct SeenLines {
    dir: Option<PathBuf>,
    snapshots: Mutex<HashMap<PathBuf, Snapshot>>,
}

impl Default for SeenLines {
    fn default() -> Self {
        Self {
            dir: None,
            snapshots: Mutex::new(HashMap::new()),
        }
    }
}

impl SeenLines {
    /// Open the per-Session seen store, loading snapshots from `dir/state.json`.
    pub fn open(dir: impl Into<PathBuf>) -> Self {
        let dir = dir.into();
        let snapshots = load(&dir);
        Self {
            dir: Some(dir),
            snapshots: Mutex::new(snapshots),
        }
    }

    pub fn record(&self, path: &Path, hash: &str, lines: impl IntoIterator<Item = u32>) {
        self.record_visible(path, hash, lines, false);
    }

    pub fn record_replacing(&self, path: &Path, hash: &str, lines: impl IntoIterator<Item = u32>) {
        self.record_visible(path, hash, lines, true);
    }

    fn record_visible(
        &self,
        path: &Path,
        hash: &str,
        lines: impl IntoIterator<Item = u32>,
        replace: bool,
    ) {
        let mut map = self.snapshots.lock().unwrap_or_else(|e| e.into_inner());
        let snapshot = map.entry(path.to_path_buf()).or_default();
        if replace || !snapshot.hash.eq_ignore_ascii_case(hash) {
            snapshot.hash = hash.to_string();
            snapshot.tag = tag_of(hash);
            snapshot.lines.clear();
        }
        snapshot.lines.extend(lines);
        self.persist(&map);
    }

    pub fn record_range(&self, path: &Path, hash: &str, start: u32, end: u32) {
        if end < start {
            return;
        }
        self.record(path, hash, start..=end);
    }

    /// `None` means this exact snapshot was never recorded. `Some(empty)` means
    /// every anchor was shown.
    pub fn unseen(&self, path: &Path, hash: &str, anchors: &[u32]) -> Option<Vec<u32>> {
        let map = self.snapshots.lock().unwrap_or_else(|e| e.into_inner());
        let snapshot = map.get(path)?;
        if !snapshot.hash.eq_ignore_ascii_case(hash) {
            return None;
        }
        Some(
            anchors
                .iter()
                .copied()
                .filter(|n| !snapshot.lines.contains(n))
                .collect(),
        )
    }

    fn persist(&self, map: &HashMap<PathBuf, Snapshot>) {
        let Some(dir) = &self.dir else {
            return;
        };
        let Ok(encoded) = serde_json::to_string(map) else {
            return;
        };
        let _ = fs::create_dir_all(dir);
        let path = dir.join("state.json");
        let tmp = dir.join(format!("state.json.tmp.{}", std::process::id()));
        if fs::write(&tmp, encoded).is_ok() {
            let _ = fs::rename(tmp, path);
        }
    }
}

fn tag_of(hash: &str) -> String {
    hash.chars().take(4).collect::<String>().to_uppercase()
}

fn load(dir: &Path) -> HashMap<PathBuf, Snapshot> {
    let Ok(text) = fs::read_to_string(dir.join("state.json")) else {
        return HashMap::new();
    };
    let Ok(raw) = serde_json::from_str::<HashMap<String, Snapshot>>(&text) else {
        return HashMap::new();
    };
    raw.into_iter()
        .map(|(path, snapshot)| (PathBuf::from(path), snapshot))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_hash_rejects_a_prefix_collision() {
        let seen = SeenLines::default();
        let path = Path::new("/project/a.rs");
        let hash = "ABCD111111111111111111111111111111111111111111111111111111111111";
        let collision = "ABCD222222222222222222222222222222222222222222222222222222222222";
        seen.record(path, hash, [1, 2]);
        assert_eq!(seen.unseen(path, hash, &[1]), Some(Vec::new()));
        assert_eq!(seen.unseen(path, collision, &[1]), None);
    }

    #[test]
    fn persisted_snapshots_reload_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = Path::new("/project/a.rs");
        let hash = "ABCD111111111111111111111111111111111111111111111111111111111111";
        SeenLines::open(dir.path()).record_range(path, hash, 1, 3);

        let restored = SeenLines::open(dir.path());
        assert_eq!(restored.unseen(path, hash, &[2, 3]), Some(Vec::new()));
        assert_eq!(restored.unseen(path, hash, &[4]), Some(vec![4]));
    }
}
