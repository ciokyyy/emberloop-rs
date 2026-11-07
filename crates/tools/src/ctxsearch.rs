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
    score: f64,
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

// shortcut: no Porter stemming or persistent index; use FTS5 when per-call spill scans become too slow.
fn tokens(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// BM25 over spill chunks, with OR semantics for query terms, as in Go's FTS5 search.
fn search(blobs: &[(String, String)], query: &str, limit: usize) -> Vec<Hit> {
    let mut terms = tokens(query);
    terms.sort();
    terms.dedup();
    if terms.is_empty() {
        return Vec::new();
    }
    let mut chunks = Vec::new();
    for (handle, text) in blobs {
        let lines: Vec<&str> = text.lines().collect();
        for (chunk, group) in lines.chunks(CHUNK_LINES).enumerate() {
            let base = chunk * CHUNK_LINES;
            let best = group
                .iter()
                .enumerate()
                .max_by_key(|(i, line)| {
                    let words = tokens(line);
                    (
                        terms.iter().filter(|term| words.contains(term)).count(),
                        usize::MAX - i,
                    )
                })
                .map_or(0, |(i, _)| i)
                + base;
            let start = best.saturating_sub(CONTEXT_LINES);
            let end = (best + CONTEXT_LINES + 1).min(lines.len());
            chunks.push((
                Hit {
                    handle: handle.clone(),
                    start: start + 1,
                    end,
                    score: 0.0,
                    snippet: lines[start..end].join(" ⏎ "),
                },
                tokens(&group.join("\n")),
            ));
        }
    }
    let count = chunks.len() as f64;
    if count == 0.0 {
        return Vec::new();
    }
    let average =
        (chunks.iter().map(|(_, words)| words.len()).sum::<usize>() as f64 / count).max(1.0);
    let weights: Vec<f64> = terms
        .iter()
        .map(|term| {
            let frequency = chunks
                .iter()
                .filter(|(_, words)| words.contains(term))
                .count() as f64;
            ((count - frequency + 0.5) / (frequency + 0.5))
                .ln()
                .max(1e-6)
        })
        .collect();
    let mut hits: Vec<Hit> = chunks
        .into_iter()
        .filter_map(|(mut hit, words)| {
            let normalization = 1.2 * (0.25 + 0.75 * words.len() as f64 / average);
            hit.score = terms
                .iter()
                .zip(&weights)
                .map(|(term, weight)| {
                    let frequency = words.iter().filter(|word| *word == term).count() as f64;
                    weight * frequency * 2.2 / (frequency + normalization)
                })
                .sum();
            (hit.score > 0.0).then_some(hit)
        })
        .collect();
    hits.sort_by(|a, b| b.score.total_cmp(&a.score));
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
    fn bm25_ranks_rare_terms_above_repeated_common_terms() {
        let blobs = vec![
            ("spam".into(), "common\n".repeat(20)),
            ("rare".into(), "common rare\n".into()),
            ("other".into(), "common\n".into()),
        ];
        let hits = search(&blobs, "common rare", 3);
        assert_eq!(hits[0].handle, "rare");
        assert_eq!(hits.len(), 3);
    }

    #[test]
    fn bm25_normalizes_length_and_tokenizes_query_punctuation() {
        let blobs = vec![
            ("long".into(), format!("needle {}", "noise ".repeat(1000))),
            ("short".into(), "needle needle\n".into()),
        ];
        assert_eq!(search(&blobs, "needle", 1)[0].handle, "short");
        assert_eq!(search(&blobs, "NEEDLE,", 1)[0].handle, "short");
        assert!(search(&blobs, "need", 3).is_empty());
        assert!(search(&blobs, "! : ()", 3).is_empty());
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
