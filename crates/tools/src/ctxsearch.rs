//! `ctx_search`: ranked snippets over the Session's spilled tool output.
//! Spill files are the handles themselves; background logs and records
//! (`<id>.log`, `<id>.json`) are not searchable output.
use std::fs;

use serde::Deserialize;
use serde_json::Value;

use crate::{ToolError, ToolResult, Toolbox};

const MAX_BYTES: usize = 4 << 10;
const CHUNK_LINES: usize = 20;
const CONTEXT_LINES: usize = 2;

#[derive(Deserialize)]
struct CtxArgs {
    queries: Vec<String>,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

struct Hit {
    handle: String,
    start: usize,
    end: usize,
    score: usize,
    snippet: String,
}

pub fn run(toolbox: &Toolbox, args: &Value) -> ToolResult<String> {
    let CtxArgs {
        queries,
        source,
        limit,
    } = serde_json::from_value(args.clone())?;
    if queries.is_empty() {
        return Err(ToolError::msg(
            "queries is required: give one or more search strings",
        ));
    }
    let limit = limit.filter(|n| *n > 0).unwrap_or(3).min(20);
    let scope = match source.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(source) => {
            let handle = source.strip_prefix("ctx:").unwrap_or("");
            let handle = handle.split(':').next().unwrap_or("");
            if toolbox.overflow.path(handle).is_none() {
                return Err(ToolError::msg(format!(
                    "invalid source handle {source:?}: want ctx:<id>"
                )));
            }
            Some(handle.to_string())
        }
        None => None,
    };
    let blobs = blobs(toolbox, scope.as_deref());
    let mut out = String::new();
    for query in &queries {
        let hits = search(&blobs, query, limit);
        if hits.is_empty() {
            out.push_str(&format!("query {query:?}: nothing matched\n"));
            continue;
        }
        out.push_str(&format!("query {query:?}: {} match(es)\n", hits.len()));
        for hit in hits {
            if out.len() >= MAX_BYTES {
                out.push_str(&format!("[budget of {MAX_BYTES} bytes reached: narrow the query, lower limit, or add source= to reach the rest; nothing is lost]\n"));
                return Ok(out);
            }
            out.push_str(&format!(
                "ctx:{}:{}-{}: {}\n",
                hit.handle, hit.start, hit.end, hit.snippet
            ));
        }
    }
    Ok(out)
}

fn blobs(toolbox: &Toolbox, only: Option<&str>) -> Vec<(String, String)> {
    let Ok(entries) = fs::read_dir(toolbox.spill_dir()) else {
        return Vec::new();
    };
    let mut blobs: Vec<(String, String)> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            (!name.contains('.') && only.is_none_or(|o| o == name)).then_some((name, entry))
        })
        .filter_map(|(name, entry)| Some((name, fs::read_to_string(entry.path()).ok()?)))
        .collect();
    blobs.sort();
    blobs
}

/// Chunks scored by how often query terms occur; best `limit` chunks, each as a window around the best line.
fn search(blobs: &[(String, String)], query: &str, limit: usize) -> Vec<Hit> {
    let terms: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    if terms.is_empty() {
        return Vec::new();
    }
    let mut hits = Vec::new();
    for (handle, text) in blobs {
        let lines: Vec<&str> = text.lines().collect();
        for (chunk, group) in lines.chunks(CHUNK_LINES).enumerate() {
            let base = chunk * CHUNK_LINES;
            let scores: Vec<usize> = group
                .iter()
                .map(|line| {
                    let line = line.to_lowercase();
                    terms
                        .iter()
                        .filter(|term| line.contains(term.as_str()))
                        .count()
                })
                .collect();
            let score: usize = scores.iter().sum();
            if score == 0 {
                continue;
            }
            let best = scores
                .iter()
                .enumerate()
                .max_by_key(|(i, s)| (**s, usize::MAX - i))
                .map_or(0, |(i, _)| i)
                + base;
            let start = best.saturating_sub(CONTEXT_LINES);
            let end = (best + CONTEXT_LINES + 1).min(lines.len());
            hits.push(Hit {
                handle: handle.clone(),
                start: start + 1,
                end,
                score,
                snippet: lines[start..end].join(" ⏎ "),
            });
        }
    }
    hits.sort_by_key(|hit| std::cmp::Reverse(hit.score));
    hits.truncate(limit);
    hits
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ToolConfig;

    fn toolbox_with(spills: &[&str]) -> (tempfile::TempDir, Toolbox) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        std::fs::create_dir_all(&root).unwrap();
        let toolbox =
            Toolbox::new(ToolConfig::new(&root, dir.path().join("session/spill"))).unwrap();
        for spill in spills {
            toolbox.overflow.write(spill).unwrap();
        }
        (dir, toolbox)
    }

    #[test]
    fn finds_ranked_snippets_with_a_page_handle() {
        let (_dir, toolbox) =
            toolbox_with(&["alpha\nbeta\nneedle here\nomega\n", "nothing\nuseful\n"]);
        let out = run(&toolbox, &serde_json::json!({"queries": ["needle"]})).unwrap();
        assert!(out.contains("1 match(es)"), "{out}");
        assert!(
            out.contains("-4: alpha ⏎ beta ⏎ needle here ⏎ omega"),
            "{out}"
        );
        let missing = run(&toolbox, &serde_json::json!({"queries": ["absent"]})).unwrap();
        assert!(missing.contains("nothing matched"));
    }

    #[test]
    fn source_scopes_to_one_handle_and_rejects_unknown() {
        let (_dir, toolbox) = toolbox_with(&["hit one\n"]);
        let handle = std::fs::read_dir(toolbox.spill_dir())
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .file_name()
            .to_string_lossy()
            .into_owned();
        let args = serde_json::json!({"queries": ["hit"], "source": format!("ctx:{handle}")});
        assert!(run(&toolbox, &args).unwrap().contains("1 match(es)"));
        assert!(run(
            &toolbox,
            &serde_json::json!({"queries": ["hit"], "source": "ctx:nope"})
        )
        .is_err());
        assert!(run(&toolbox, &serde_json::json!({"queries": []})).is_err());
    }
}
