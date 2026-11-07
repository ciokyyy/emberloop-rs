use serde_json::Value;

/// Token counts read from one generation response. The meter never sums across
/// requests: the last request's input is the context size the UI shows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    /// The whole prompt of the request (the context size). See `input_total`.
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Input tokens the upstream served from its prompt cache, when it says so.
    pub cached_tokens: u64,
}

/// Extract usage from a Cloud Code SSE stream or a plain JSON response.
/// Later payloads win, so a final totals chunk beats earlier partials.
pub fn extract_usage(bytes: &[u8]) -> Option<Usage> {
    let text = String::from_utf8_lossy(bytes);
    let mut merged = Usage::default();
    let mut found = false;
    for payload in json_payloads(&text) {
        if let Some(usage) = usage_from_payload(&payload) {
            found = true;
            if usage.cached_tokens > 0 {
                merged.cached_tokens = usage.cached_tokens;
            }
            if usage.input_tokens > 0 {
                merged.input_tokens = usage.input_tokens;
            }
            if usage.output_tokens > 0 {
                merged.output_tokens = usage.output_tokens;
            }
        }
    }
    found.then_some(merged)
}

/// The JSON documents a response body holds: the whole body, or one per SSE
/// `data:` line.
pub(crate) fn payloads(text: &str) -> Vec<Value> {
    json_payloads(text)
}

fn json_payloads(text: &str) -> Vec<Value> {
    if let Ok(value) = serde_json::from_str::<Value>(text.trim()) {
        return vec![value];
    }
    text.lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(str::trim)
        .filter(|data| !data.is_empty() && *data != "[DONE]")
        .filter_map(|data| serde_json::from_str::<Value>(data).ok())
        .collect()
}

fn usage_from_payload(value: &Value) -> Option<Usage> {
    for root in [
        value.get("response").unwrap_or(value),
        value.get("message").unwrap_or(value),
        value,
    ] {
        if let Some(usage) = gemini_usage(root) {
            return Some(usage);
        }
        if let Some(usage) = openai_usage(root) {
            return Some(usage);
        }
    }
    None
}

fn gemini_usage(root: &Value) -> Option<Usage> {
    let usage = root.get("usageMetadata")?;
    let input = number(usage, "promptTokenCount");
    let output = number(usage, "candidatesTokenCount");
    let total = number(usage, "totalTokenCount");
    if input == 0 && output == 0 && total == 0 {
        return None;
    }
    Some(Usage {
        input_tokens: input,
        output_tokens: output,
        cached_tokens: number(usage, "cachedContentTokenCount"),
    })
}

fn openai_usage(root: &Value) -> Option<Usage> {
    let usage = root.get("usage")?;
    let input = input_total(usage);
    let output = number(usage, "completion_tokens").max(number(usage, "output_tokens"));
    if input == 0 && output == 0 {
        return None;
    }
    Some(Usage {
        input_tokens: input,
        output_tokens: output,
        cached_tokens: cached_tokens(usage),
    })
}

/// The prompt size of a request. Anthropic's `input_tokens` leaves out its cache reads and
/// writes, so the total adds them back; OpenAI counts its cached tokens inside the input total
/// already. Gemini: `cachedContentTokenCount` is left out of the total on purpose. Whether
/// `promptTokenCount` includes it is UNVERIFIED, so the reported count is kept as it is.
fn input_total(usage: &Value) -> u64 {
    if !is_anthropic(usage) {
        return number(usage, "prompt_tokens").max(number(usage, "input_tokens"));
    }
    // A message_delta may carry only the cache counts; without `input_tokens` it is no total.
    if usage.get("input_tokens").is_none() {
        return 0;
    }
    number(usage, "input_tokens")
        + number(usage, "cache_read_input_tokens")
        + number(usage, "cache_creation_input_tokens")
}

/// Anthropic is the only wire whose usage carries cache counts beside `input_tokens`.
fn is_anthropic(usage: &Value) -> bool {
    usage.get("cache_read_input_tokens").is_some()
        || usage.get("cache_creation_input_tokens").is_some()
}

/// Where each wire reports the prefix it served from cache. OpenAI-style usage carries
/// it under `*_tokens_details`; Anthropic under `cache_read_input_tokens`.
fn cached_tokens(usage: &Value) -> u64 {
    let details = |key: &str| number(usage.pointer(key).unwrap_or(usage), "cached_tokens");
    number(usage, "cache_read_input_tokens")
        .max(number(usage, "cached_tokens"))
        .max(details("/prompt_tokens_details"))
        .max(details("/input_tokens_details"))
}

fn number(value: &Value, key: &str) -> u64 {
    value.get(key).and_then(Value::as_u64).unwrap_or(0)
}

pub fn thinking_delta(payload: &Value) -> Option<String> {
    let text = payload
        .pointer("/choices/0/delta/reasoning_content")
        .or_else(|| payload.pointer("/choices/0/delta/reasoning"))
        .or_else(|| payload.pointer("/delta/thinking"))
        .or_else(|| {
            let kind = payload.get("type").and_then(Value::as_str)?;
            (kind == "response.reasoning_summary_text.delta"
                || kind == "response.reasoning_text.delta")
                .then(|| payload.get("delta"))
                .flatten()
        })?
        .as_str()?;
    (!text.is_empty()).then(|| text.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_reasoning_text_from_each_streaming_wire() {
        let chat = serde_json::json!({"choices":[{"delta":{"reasoning_content":"hmm"}}]});
        assert_eq!(thinking_delta(&chat).as_deref(), Some("hmm"));
        let anthropic = serde_json::json!({"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"so"}});
        assert_eq!(thinking_delta(&anthropic).as_deref(), Some("so"));
        let responses =
            serde_json::json!({"type":"response.reasoning_summary_text.delta","delta":"ok"});
        assert_eq!(thinking_delta(&responses).as_deref(), Some("ok"));
        let plain =
            serde_json::json!({"choices":[{"delta":{"content":"hi","reasoning_content":""}}]});
        assert_eq!(thinking_delta(&plain), None);
    }

    #[test]
    fn reads_gemini_usage_from_the_last_sse_chunk() {
        let body = "data: {\"response\":{\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"hi\"}]}}]}}\n\n\
                    data: {\"response\":{\"usageMetadata\":{\"promptTokenCount\":120,\"candidatesTokenCount\":7}}}\n\n";
        assert_eq!(
            extract_usage(body.as_bytes()),
            Some(Usage {
                input_tokens: 120,
                output_tokens: 7,
                cached_tokens: 0
            })
        );
    }

    #[test]
    fn reads_usage_from_a_plain_json_response() {
        let body =
            r#"{"candidates":[],"usageMetadata":{"promptTokenCount":9,"candidatesTokenCount":3}}"#;
        assert_eq!(
            extract_usage(body.as_bytes()),
            Some(Usage {
                input_tokens: 9,
                output_tokens: 3,
                cached_tokens: 0
            })
        );
    }

    #[test]
    fn reads_openai_usage() {
        let body = r#"{"usage":{"prompt_tokens":44,"completion_tokens":2}}"#;
        assert_eq!(
            extract_usage(body.as_bytes()),
            Some(Usage {
                input_tokens: 44,
                output_tokens: 2,
                cached_tokens: 0
            })
        );
    }

    #[test]
    fn reads_anthropic_and_responses_usage_across_sse_chunks() {
        let anthropic = "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":31,\"output_tokens\":1}}}\n\n\
                         data: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":14}}\n\n";
        assert_eq!(
            extract_usage(anthropic.as_bytes()),
            Some(Usage {
                input_tokens: 31,
                output_tokens: 14,
                cached_tokens: 0
            })
        );
        let responses =
            "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":52,\"output_tokens\":8}}}\n\n";
        assert_eq!(
            extract_usage(responses.as_bytes()),
            Some(Usage {
                input_tokens: 52,
                output_tokens: 8,
                cached_tokens: 0
            })
        );
    }

    #[test]
    fn reads_cached_tokens_from_each_wire() {
        let openai = r#"{"usage":{"prompt_tokens":44,"completion_tokens":2,"prompt_tokens_details":{"cached_tokens":30}}}"#;
        assert_eq!(
            extract_usage(openai.as_bytes()).map(|u| u.cached_tokens),
            Some(30)
        );
        let anthropic = "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":31,\"cache_read_input_tokens\":20}}}\n\n";
        assert_eq!(
            extract_usage(anthropic.as_bytes()).map(|u| u.cached_tokens),
            Some(20)
        );
        let gemini = r#"{"usageMetadata":{"promptTokenCount":120,"candidatesTokenCount":7,"cachedContentTokenCount":15}}"#;
        assert_eq!(
            extract_usage(gemini.as_bytes()).map(|u| u.cached_tokens),
            Some(15)
        );
    }

    #[test]
    fn anthropic_input_is_the_whole_prompt_with_cache_reads_and_writes() {
        // Anthropic's `input_tokens` excludes cache reads and writes, so the context is their sum.
        let body = "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":2000,\"cache_read_input_tokens\":80000,\"cache_creation_input_tokens\":5000,\"output_tokens\":1}}}\n\n";
        assert_eq!(
            extract_usage(body.as_bytes()),
            Some(Usage {
                input_tokens: 87000,
                output_tokens: 1,
                cached_tokens: 80000,
            })
        );
    }

    #[test]
    fn anthropic_message_delta_without_input_keeps_the_context_total() {
        let body = "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":2000,\"cache_read_input_tokens\":80000,\"cache_creation_input_tokens\":5000,\"output_tokens\":1}}}\n\n\
                    data: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":14}}\n\n";
        assert_eq!(
            extract_usage(body.as_bytes()),
            Some(Usage {
                input_tokens: 87000,
                output_tokens: 14,
                cached_tokens: 80000,
            })
        );
    }

    #[test]
    fn a_response_without_usage_yields_none() {
        assert_eq!(extract_usage(b"data: {\"response\":{}}\n\n"), None);
        assert_eq!(extract_usage(b"data: [DONE]\n\n"), None);
    }
}
