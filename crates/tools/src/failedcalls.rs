use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::mcp::strip_meta_keys;
use crate::{ToolError, ToolResult, Toolbox};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FailedCallRecord {
    pub tool: String,
    pub args: Value,
}

#[derive(Debug, Deserialize)]
struct RetryCallArgs {
    id: u64,
    #[serde(default)]
    set: Option<Map<String, Value>>,
    #[serde(default)]
    patches: Vec<Patch>,
}

#[derive(Debug, Deserialize)]
struct Patch {
    old: String,
    new: String,
}

#[derive(Clone, Debug)]
pub struct FailedCallStore {
    dir: PathBuf,
}

impl FailedCallStore {
    pub fn new(spill_dir: &Path) -> Self {
        Self {
            dir: spill_dir.join("failed_calls"),
        }
    }

    pub fn record(&self, tool: &str, args: &Value) -> ToolResult<u64> {
        fs::create_dir_all(&self.dir)?;
        let record = FailedCallRecord {
            tool: tool.to_string(),
            args: strip_meta_keys(args),
        };
        let payload = serde_json::to_vec_pretty(&record)?;
        let max_id = max_stored_id(&self.dir);
        for offset in 0..1000 {
            let id = max_id + 1 + offset;
            let path = self.entry_path(id);
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut file) => {
                    if let Err(error) = file.write_all(&payload) {
                        let _ = fs::remove_file(&path);
                        return Err(error.into());
                    }
                    return Ok(id);
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        }
        Err(ToolError::msg("could not allocate failed call id"))
    }

    pub fn load(&self, id: u64) -> ToolResult<FailedCallRecord> {
        if id == 0 {
            return Err(ToolError::msg("retry_call id must be >= 1"));
        }
        let path = self.entry_path(id);
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(ToolError::msg(format!("failed call #{id} was not found")));
            }
            Err(error) => return Err(error.into()),
        };
        Ok(serde_json::from_slice(&bytes)?)
    }

    fn entry_path(&self, id: u64) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }
}

fn max_stored_id(dir: &Path) -> u64 {
    fs::read_dir(dir)
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            entry
                .file_name()
                .to_str()?
                .strip_suffix(".json")?
                .parse::<u64>()
                .ok()
        })
        .max()
        .unwrap_or(0)
}

// Boxed to break async Send-checking recursion with Toolbox::call.
pub fn run_retry<'a>(
    toolbox: &'a Toolbox,
    args: &'a Value,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult<String>> + Send + 'a>> {
    Box::pin(async move {
        let parsed: RetryCallArgs = serde_json::from_value(args.clone())?;
        let record = toolbox.failed_calls().load(parsed.id)?;
        if record.tool == "retry_call" {
            return Err(ToolError::msg("cannot retry retry_call recursively"));
        }
        let edited = apply_edits(record.args, parsed.set, &parsed.patches)?;
        toolbox.call(&record.tool, &edited).await
    })
}

fn apply_edits(
    mut args: Value,
    set: Option<Map<String, Value>>,
    patches: &[Patch],
) -> ToolResult<Value> {
    if let Some(overrides) = set {
        let obj = args.as_object_mut().ok_or_else(|| {
            ToolError::msg("retry_call `set` requires stored arguments to be a JSON object")
        })?;
        for (key, value) in overrides {
            if value.is_null() {
                obj.remove(&key);
            } else {
                obj.insert(key, value);
            }
        }
    }
    for patch in patches {
        apply_patch(&mut args, patch)?;
    }
    Ok(args)
}

fn apply_patch(args: &mut Value, patch: &Patch) -> ToolResult<()> {
    if patch.old.is_empty() {
        return Err(ToolError::msg("retry_call patch `old` must not be empty"));
    }
    let mut strings = Vec::new();
    collect_strings_mut(args, &mut strings);
    let count: usize = strings
        .iter()
        .map(|text| text.matches(&patch.old).count())
        .sum();
    if count == 0 {
        return Err(ToolError::msg(format!(
            "retry_call patch matched 0 occurrences of {:?}",
            preview(&patch.old)
        )));
    }
    if count > 1 {
        return Err(ToolError::msg(format!(
            "retry_call patch matched {count} occurrences of {:?} (must match exactly once)",
            preview(&patch.old)
        )));
    }
    if let Some(target) = strings.into_iter().find(|text| text.contains(&patch.old)) {
        *target = target.replacen(&patch.old, &patch.new, 1);
    }
    Ok(())
}

fn collect_strings_mut<'a>(value: &'a mut Value, out: &mut Vec<&'a mut String>) {
    match value {
        Value::String(text) => out.push(text),
        Value::Array(items) => {
            for item in items {
                collect_strings_mut(item, out);
            }
        }
        Value::Object(map) => {
            for val in map.values_mut() {
                collect_strings_mut(val, out);
            }
        }
        _ => {}
    }
}

fn preview(text: &str) -> String {
    let mut out: String = text.chars().take(60).collect();
    if text.chars().count() > 60 {
        out.push_str("...");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn store_persists_records_with_monotonic_ids_and_strips_meta_keys() {
        let dir = tempfile::tempdir().unwrap();
        let store = FailedCallStore::new(dir.path());
        let id1 = store
            .record(
                "spawn",
                &json!({"role": "worker", "text": "hi", "toolAction": "x", "toolSummary": "y"}),
            )
            .unwrap();
        let id2 = store
            .record("steer", &json!({"id": "ses_1", "text": "next"}))
            .unwrap();
        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
        let rec1 = store.load(1).unwrap();
        assert_eq!(rec1.tool, "spawn");
        assert_eq!(rec1.args, json!({"role": "worker", "text": "hi"}));
    }

    #[test]
    fn apply_edits_sets_removes_null_and_patches_exact_once() {
        let args = json!({
            "role": "worker",
            "ticketPath": "wrong.md",
            "text": "Fix foo in bar.rs",
            "extra": ["Fix baz"]
        });
        let mut set = Map::new();
        set.insert("ticketPath".into(), Value::Null);
        set.insert("tier".into(), json!("mid"));
        let patches = vec![
            Patch {
                old: "foo in bar.rs".into(),
                new: "qux in bar.rs".into(),
            },
            Patch {
                old: "baz".into(),
                new: "quux".into(),
            },
        ];
        let edited = apply_edits(args, Some(set), &patches).unwrap();
        assert_eq!(
            edited,
            json!({
                "role": "worker",
                "tier": "mid",
                "text": "Fix qux in bar.rs",
                "extra": ["Fix quux"]
            })
        );
    }

    #[test]
    fn apply_edits_rejects_missing_empty_and_ambiguous_patches() {
        let args = json!({"text": "dup dup", "other": "abc"});
        let missing = apply_edits(
            args.clone(),
            None,
            &[Patch {
                old: "nope".into(),
                new: "x".into(),
            }],
        )
        .unwrap_err()
        .to_string();
        assert!(missing.contains("0 occurrences"), "{missing}");

        let ambiguous = apply_edits(
            args.clone(),
            None,
            &[Patch {
                old: "dup".into(),
                new: "x".into(),
            }],
        )
        .unwrap_err()
        .to_string();
        assert!(ambiguous.contains("2 occurrences"), "{ambiguous}");

        let empty = apply_edits(
            args,
            None,
            &[Patch {
                old: String::new(),
                new: "x".into(),
            }],
        )
        .unwrap_err()
        .to_string();
        assert!(empty.contains("must not be empty"), "{empty}");
    }

    #[test]
    fn load_rejects_zero_and_missing_id() {
        let dir = tempfile::tempdir().unwrap();
        let store = FailedCallStore::new(dir.path());
        let zero = store.load(0).unwrap_err().to_string();
        assert!(zero.contains(">= 1"), "{zero}");
        let missing = store.load(999).unwrap_err().to_string();
        assert!(missing.contains("was not found"), "{missing}");
    }

    #[test]
    fn record_allocates_around_preexisting_entry() {
        let dir = tempfile::tempdir().unwrap();
        let store = FailedCallStore::new(dir.path());
        fs::create_dir_all(dir.path().join("failed_calls")).unwrap();
        fs::write(dir.path().join("failed_calls/3.json"), "{}").unwrap();
        let id = store.record("spawn", &json!({"role": "worker"})).unwrap();
        assert_eq!(id, 4);
    }

    #[test]
    fn run_retry_rejects_recursive_retry_call_record() {
        let dir = tempfile::tempdir().unwrap();
        let toolbox =
            Toolbox::new(crate::ToolConfig::new(dir.path(), dir.path().join("spill"))).unwrap();
        let id = toolbox
            .failed_calls()
            .record("retry_call", &json!({"id": 1}))
            .unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let err = runtime
            .block_on(run_retry(&toolbox, &json!({"id": id})))
            .unwrap_err()
            .to_string();
        assert!(err.contains("recursively"), "{err}");
    }
}
