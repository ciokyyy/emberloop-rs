use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const MAX_VERSIONS: usize = 4;

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Snapshot {
    #[serde(default)]
    pub tag: String,
    #[serde(default)]
    pub hash: String,
    #[serde(default)]
    pub lines: BTreeSet<u32>,
    #[serde(default)]
    pub content_lines: Vec<String>,
}

/// Lines a tool view showed, keyed by path and full snapshot hash.
///
/// The four-hex tag is only a display label. `hash` is the full sha256, so a
/// 16-bit prefix collision cannot make a stale edit look current. Up to 4
/// historical versions are kept per path so `edit` can rebase a stale tag when
/// the file changed outside the target lines.
pub struct SeenLines {
    dir: Option<PathBuf>,
    snapshots: Mutex<HashMap<PathBuf, Vec<Snapshot>>>,
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
        self.record_visible(path, hash, None, lines, false);
    }

    pub fn record_with_content(
        &self,
        path: &Path,
        hash: &str,
        content_lines: Vec<String>,
        lines: impl IntoIterator<Item = u32>,
        replace: bool,
    ) {
        self.record_visible(path, hash, Some(content_lines), lines, replace);
    }

    pub fn record_replacing(&self, path: &Path, hash: &str, lines: impl IntoIterator<Item = u32>) {
        self.record_visible(path, hash, None, lines, true);
    }

    fn record_visible(
        &self,
        path: &Path,
        hash: &str,
        content_lines: Option<Vec<String>>,
        lines: impl IntoIterator<Item = u32>,
        replace: bool,
    ) {
        let mut map = self.snapshots.lock().unwrap_or_else(|e| e.into_inner());
        let versions = map.entry(path.to_path_buf()).or_default();
        let idx = versions
            .iter()
            .position(|v| v.hash.eq_ignore_ascii_case(hash));
        let snap = if let Some(i) = idx {
            let mut s = versions.remove(i);
            if replace {
                s.lines.clear();
            }
            versions.insert(0, s);
            &mut versions[0]
        } else {
            versions.insert(
                0,
                Snapshot {
                    tag: tag_of(hash),
                    hash: hash.to_string(),
                    lines: BTreeSet::new(),
                    content_lines: Vec::new(),
                },
            );
            if versions.len() > MAX_VERSIONS {
                versions.truncate(MAX_VERSIONS);
            }
            &mut versions[0]
        };
        if let Some(cl) = content_lines {
            snap.content_lines = cl;
        }
        snap.lines.extend(lines);
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
        let versions = map.get(path)?;
        let snapshot = versions
            .iter()
            .find(|v| v.hash.eq_ignore_ascii_case(hash))?;
        Some(
            anchors
                .iter()
                .copied()
                .filter(|n| !snapshot.lines.contains(n))
                .collect(),
        )
    }

    pub fn lookup_tag(&self, path: &Path, tag: &str) -> Option<Snapshot> {
        let map = self.snapshots.lock().unwrap_or_else(|e| e.into_inner());
        let versions = map.get(path)?;
        versions
            .iter()
            .find(|v| v.tag.eq_ignore_ascii_case(tag) && !v.content_lines.is_empty())
            .cloned()
    }

    fn persist(&self, map: &HashMap<PathBuf, Vec<Snapshot>>) {
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

fn load(dir: &Path) -> HashMap<PathBuf, Vec<Snapshot>> {
    let Ok(text) = fs::read_to_string(dir.join("state.json")) else {
        return HashMap::new();
    };
    if let Ok(raw) = serde_json::from_str::<HashMap<String, Vec<Snapshot>>>(&text) {
        return raw
            .into_iter()
            .map(|(path, versions)| (PathBuf::from(path), versions))
            .collect();
    }
    if let Ok(legacy) = serde_json::from_str::<HashMap<String, Snapshot>>(&text) {
        return legacy
            .into_iter()
            .map(|(path, snap)| (PathBuf::from(path), vec![snap]))
            .collect();
    }
    HashMap::new()
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
