use std::fs;
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{ToolError, ToolResult, Toolbox};

const DEFAULT_COUNT: usize = 10;
const MAX_COUNT: usize = 20;
const CACHE_TTL_SECS: u64 = 15 * 60;
const DEFAULT_BRAVE_ENDPOINT: &str = "https://api.search.brave.com";
const DEFAULT_EXA_ENDPOINT: &str = "https://api.exa.ai";

#[derive(Deserialize)]
struct SearchInput {
    query: String,
    #[serde(default)]
    count: Option<usize>,
    #[serde(default)]
    freshness: Option<String>,
    #[serde(default)]
    domains: Vec<String>,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    fresh: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SearchResultItem {
    title: String,
    url: String,
    snippet: String,
    published: String,
}

pub async fn run(toolbox: &Toolbox, args: &Value) -> ToolResult<String> {
    let input: SearchInput = serde_json::from_value(args.clone())?;
    let query = input.query.split_whitespace().collect::<Vec<_>>().join(" ");
    if query.is_empty() {
        return Err(ToolError::msg("query cannot be empty"));
    }
    let count = input.count.unwrap_or(DEFAULT_COUNT).clamp(1, MAX_COUNT);
    let freshness = input
        .freshness
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if !matches!(freshness.as_str(), "" | "day" | "week" | "month" | "year") {
        return Err(ToolError::msg(format!(
            "unknown freshness {freshness:?} (have day, week, month, year)"
        )));
    }
    let provider = input
        .provider
        .unwrap_or_else(|| "auto".to_string())
        .trim()
        .to_ascii_lowercase();
    if !matches!(provider.as_str(), "auto" | "brave" | "exa" | "deepseek") {
        return Err(ToolError::msg(format!(
            "unknown provider {provider:?} (have deepseek, brave, exa, auto)"
        )));
    }
    let domains: Vec<String> = input
        .domains
        .iter()
        .map(|d| normalise_domain(d))
        .filter(|d| !d.is_empty())
        .collect();
    let cache_key = compute_cache_key(&query, count, &freshness, &domains, &provider);
    let cache_path = toolbox
        .spill_dir()
        .join(format!("websearch-{cache_key}.txt"));
    if !input.fresh {
        if let Some(cached) = read_cache(&cache_path) {
            return Ok(format!("{cached}\n[cached]"));
        }
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| ToolError::msg(format!("http client: {e}")))?;
    let (items, answered_by, notes) = execute_providers(
        toolbox, &client, &query, count, &freshness, &domains, &provider,
    )
    .await?;
    let rendered = render_results(&query, &items, &answered_by, &notes);
    let _ = fs::create_dir_all(toolbox.spill_dir());
    let _ = fs::write(&cache_path, &rendered);
    Ok(rendered)
}

async fn execute_providers(
    toolbox: &Toolbox,
    client: &reqwest::Client,
    query: &str,
    count: usize,
    freshness: &str,
    domains: &[String],
    provider: &str,
) -> ToolResult<(Vec<SearchResultItem>, String, Vec<String>)> {
    let chain: Vec<&str> = match provider {
        "brave" => vec!["brave"],
        "exa" => vec!["exa"],
        _ => vec!["brave", "exa"],
    };
    let mut notes = Vec::new();
    let mut last_err = None;
    for name in chain {
        match name {
            "brave" => {
                let key = toolbox.tool_key("BRAVE_API_KEY").unwrap_or_default();
                if key.trim().is_empty() {
                    if provider == "brave" {
                        return Err(ToolError::msg("BRAVE_API_KEY is not set"));
                    }
                    notes.push("brave skipped (no BRAVE_API_KEY)".to_string());
                    continue;
                }
                match search_brave(client, key.trim(), query, count, freshness, domains).await {
                    Ok(items) if !items.is_empty() || provider == "brave" => {
                        return Ok((items, "brave".to_string(), notes));
                    }
                    Ok(_) => notes.push("brave returned 0 results".to_string()),
                    Err(e) => {
                        notes.push(format!("brave failed: {e}"));
                        last_err = Some(e);
                    }
                }
            }
            "exa" => {
                let key = toolbox.tool_key("EXA_API_KEY").unwrap_or_default();
                if key.trim().is_empty() {
                    if provider == "exa" {
                        return Err(ToolError::msg("EXA_API_KEY is not set"));
                    }
                    notes.push("exa skipped (no EXA_API_KEY)".to_string());
                    continue;
                }
                match search_exa(client, key.trim(), query, count, domains).await {
                    Ok(items) => return Ok((items, "exa".to_string(), notes)),
                    Err(e) => {
                        notes.push(format!("exa failed: {e}"));
                        last_err = Some(e);
                    }
                }
            }
            _ => {}
        }
    }
    if let Some(err) = last_err {
        return Err(err);
    }
    Err(ToolError::msg(format!(
        "no websearch provider configured ({}); set BRAVE_API_KEY or EXA_API_KEY",
        notes.join(", ")
    )))
}

async fn search_brave(
    client: &reqwest::Client,
    key: &str,
    query: &str,
    count: usize,
    freshness: &str,
    domains: &[String],
) -> ToolResult<Vec<SearchResultItem>> {
    let base = std::env::var("EL_BRAVE_ENDPOINT")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_BRAVE_ENDPOINT.to_string());
    let endpoint = format!("{}/res/v1/web/search", base.trim_end_matches('/'));
    let mut query_params = vec![("q", query.to_string()), ("count", count.to_string())];
    let bf = match freshness {
        "day" => "pd",
        "week" => "pw",
        "month" => "pm",
        "year" => "py",
        _ => "",
    };
    if !bf.is_empty() {
        query_params.push(("freshness", bf.to_string()));
    }
    let resp = client
        .get(&endpoint)
        .query(&query_params)
        .header("Accept", "application/json")
        .header("X-Subscription-Token", key)
        .send()
        .await
        .map_err(|e| ToolError::msg(format!("brave request: {e}")))?;
    if !resp.status().is_success() {
        return Err(ToolError::msg(format!("brave HTTP {}", resp.status())));
    }
    let doc: Value = resp
        .json()
        .await
        .map_err(|e| ToolError::msg(format!("brave json: {e}")))?;
    let items = parse_brave_items(&doc);
    Ok(filter_domains(items, domains))
}

fn parse_brave_items(doc: &Value) -> Vec<SearchResultItem> {
    let arr = doc
        .pointer("/web/results")
        .and_then(Value::as_array)
        .or_else(|| doc.get("results").and_then(Value::as_array));
    let Some(list) = arr else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in list {
        let url = first_str(item, &["url", "link"]);
        if url.is_empty() {
            continue;
        }
        out.push(SearchResultItem {
            title: first_str(item, &["title", "name"]),
            url,
            snippet: first_str(item, &["description", "snippet", "text"]),
            published: first_str(item, &["date", "published", "page_age", "age"]),
        });
    }
    out
}

async fn search_exa(
    client: &reqwest::Client,
    key: &str,
    query: &str,
    count: usize,
    domains: &[String],
) -> ToolResult<Vec<SearchResultItem>> {
    let base = std::env::var("EL_EXA_ENDPOINT")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_EXA_ENDPOINT.to_string());
    let endpoint = format!("{}/search", base.trim_end_matches('/'));
    let mut body = serde_json::json!({
        "query": query,
        "numResults": count,
    });
    if !domains.is_empty() {
        body["includeDomains"] = serde_json::json!(domains);
    }
    let resp = client
        .post(&endpoint)
        .header("Accept", "application/json")
        .header("x-api-key", key)
        .json(&body)
        .send()
        .await
        .map_err(|e| ToolError::msg(format!("exa request: {e}")))?;
    if !resp.status().is_success() {
        return Err(ToolError::msg(format!("exa HTTP {}", resp.status())));
    }
    let doc: Value = resp
        .json()
        .await
        .map_err(|e| ToolError::msg(format!("exa json: {e}")))?;
    let Some(list) = doc.get("results").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for item in list {
        let url = first_str(item, &["url"]);
        if url.is_empty() {
            continue;
        }
        out.push(SearchResultItem {
            title: first_str(item, &["title", "name"]),
            url,
            snippet: first_str(item, &["text", "snippet", "summary", "description"]),
            published: first_str(item, &["publishedDate", "published_date", "date"]),
        });
    }
    Ok(filter_domains(out, domains))
}

fn first_str(item: &Value, keys: &[&str]) -> String {
    for k in keys {
        if let Some(s) = item
            .get(*k)
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
        {
            return s.trim().to_string();
        }
    }
    String::new()
}

fn normalise_domain(raw: &str) -> String {
    let mut s = raw.trim().to_ascii_lowercase();
    if let Some(rest) = s.strip_prefix("https://") {
        s = rest.to_string();
    } else if let Some(rest) = s.strip_prefix("http://") {
        s = rest.to_string();
    }
    if let Some((host, _)) = s.split_once('/') {
        s = host.to_string();
    }
    s.trim_start_matches("www.").to_string()
}

fn filter_domains(items: Vec<SearchResultItem>, domains: &[String]) -> Vec<SearchResultItem> {
    if domains.is_empty() {
        return items;
    }
    items
        .into_iter()
        .filter(|item| {
            reqwest::Url::parse(&item.url)
                .ok()
                .and_then(|u| u.host_str().map(|h| h.to_ascii_lowercase()))
                .is_some_and(|host| {
                    let clean = host.trim_start_matches("www.");
                    domains
                        .iter()
                        .any(|d| clean == d || clean.ends_with(&format!(".{d}")))
                })
        })
        .collect()
}

fn compute_cache_key(
    query: &str,
    count: usize,
    freshness: &str,
    domains: &[String],
    provider: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(
        format!(
            "{query}\0{count}\0{freshness}\0{}\0{provider}",
            domains.join(",")
        )
        .as_bytes(),
    );
    hex::encode(hasher.finalize())
}

fn read_cache(path: &std::path::Path) -> Option<String> {
    let meta = fs::metadata(path).ok()?;
    let modified = meta.modified().ok()?;
    let age = SystemTime::now().duration_since(modified).ok()?;
    if age > Duration::from_secs(CACHE_TTL_SECS) {
        return None;
    }
    fs::read_to_string(path).ok()
}

fn render_results(
    query: &str,
    items: &[SearchResultItem],
    provider: &str,
    notes: &[String],
) -> String {
    let mut out = format!(
        "websearch {:?}: {} result(s) via {provider}\n",
        query,
        items.len()
    );
    for (idx, item) in items.iter().enumerate() {
        let title = if item.title.is_empty() {
            &item.url
        } else {
            &item.title
        };
        out.push_str(&format!("\n{}. {title}\n   {}\n", idx + 1, item.url));
        if !item.published.is_empty() {
            out.push_str(&format!("   published: {}\n", item.published));
        }
        if !item.snippet.is_empty() {
            out.push_str(&format!("   {}\n", item.snippet));
        }
    }
    if !notes.is_empty() {
        out.push_str(&format!("\nnotes: {}\n", notes.join("; ")));
    }
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_brave_items_extracts_results() {
        let doc = json!({
            "web": {
                "results": [
                    {
                        "title": "Rust Lang",
                        "url": "https://www.rust-lang.org",
                        "description": "Fast and reliable",
                        "page_age": "2026-01-01"
                    }
                ]
            }
        });
        let items = parse_brave_items(&doc);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "Rust Lang");
        assert_eq!(items[0].url, "https://www.rust-lang.org");
        assert_eq!(items[0].snippet, "Fast and reliable");
    }
}
