//! Background skill review. After a Session's final answer, the proxy replays the request it just
//! sent, plus the review prompt, as a separate non-streaming conversation and runs the model's tool
//! calls through the daemon's executor. Nothing of it reaches the harness or the Session history.
//!
//! The fork body is the exact body of the last request with native items appended, never a
//! re-serialised copy, so upstream's prompt cache serves the whole shared prefix.

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::server::request_body_mut;
use crate::wire::{parse_args_field, WireKind};

/// Model rounds one review may take.
pub const MAX_ROUNDS: usize = 16;

/// The tool whose call by the Session resets the count: a skill was just written.
const SKILL_WRITE: &str = "skill_manage";

/// Runs one tool of a Session for the review. The daemon implements it and owns the whitelist;
/// this crate has no workspace tools.
#[async_trait]
pub trait ReviewTools: Send + Sync + std::fmt::Debug {
    /// `Err` is the text the model sees as the tool's failure.
    async fn call(&self, session_id: &str, name: &str, args: &Value) -> Result<String, String>;
}

/// One Session's review counter, in memory.
#[derive(Debug, Default)]
pub struct ReviewState {
    count: u32,
    reviewing: bool,
}

impl ReviewState {
    /// A round of the Session's request ended. `forwarded` are the tool calls it handed to the
    /// harness; none means the final answer. Returns whether a review starts now.
    pub fn round_ended(&mut self, forwarded: &[String], interval: u32) -> bool {
        if !forwarded.is_empty() {
            self.count = if forwarded.iter().any(|name| name.ends_with(SKILL_WRITE)) {
                0
            } else {
                self.count.saturating_add(1)
            };
            return false;
        }
        if interval == 0 || self.reviewing || self.count < interval {
            return false;
        }
        self.count = 0;
        self.reviewing = true;
        true
    }

    pub fn finished(&mut self) {
        self.reviewing = false;
    }
}

/// Whether the request offers the skill tool; a side request such as a title does not.
pub fn offers_skill_tool(request: &Value) -> bool {
    let mut request = request.clone();
    request_body_mut(&mut request)
        .get("tools")
        .is_some_and(|tools| tools.to_string().contains(SKILL_WRITE))
}

#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    pub id: String,
    pub name: String,
    pub args: Value,
}

/// The fork's first request: `url` and `body` made non-streaming, with the review prompt added.
pub fn fork_request(wire: WireKind, url: &str, body: &Value, prompt: &str) -> (String, Value) {
    let mut body = body.clone();
    let mut url = url.to_string();
    if wire == WireKind::Google {
        url = url
            .replace(":streamGenerateContent", ":generateContent")
            .replace("?alt=sse&", "?")
            .replace("?alt=sse", "")
            .replace("&alt=sse", "");
    } else if let Some(map) = body.as_object_mut() {
        map.insert("stream".into(), Value::Bool(false));
        map.remove("stream_options");
    }
    push(wire, &mut body, user_text(wire, prompt));
    (url, body)
}

/// The tool calls in a non-streaming response.
pub fn calls(wire: WireKind, response: &Value) -> Vec<Call> {
    match wire {
        WireKind::Anthropic => array(response.get("content"))
            .iter()
            .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_use"))
            .map(|block| Call {
                id: text_of(block, "id"),
                name: text_of(block, "name"),
                args: block.get("input").cloned().unwrap_or_else(|| json!({})),
            })
            .collect(),
        WireKind::OpenAiChat => array(response.pointer("/choices/0/message/tool_calls"))
            .iter()
            .map(|call| Call {
                id: text_of(call, "id"),
                name: call
                    .pointer("/function/name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                args: parse_args_field(call.pointer("/function/arguments")),
            })
            .collect(),
        WireKind::OpenAiResponses => array(response.get("output"))
            .iter()
            .filter(|item| item.get("type").and_then(Value::as_str) == Some("function_call"))
            .map(|item| Call {
                id: text_of(item, "call_id"),
                name: text_of(item, "name"),
                args: parse_args_field(item.get("arguments")),
            })
            .collect(),
        WireKind::Google => array(google_content(response).and_then(|c| c.get("parts")))
            .iter()
            .filter_map(|part| part.get("functionCall"))
            .map(|call| Call {
                id: text_of(call, "id"),
                name: text_of(call, "name"),
                args: call.get("args").cloned().unwrap_or_else(|| json!({})),
            })
            .collect(),
    }
}

/// Appends the model's response and the results of its calls, in the wire's own items.
pub fn append_round(
    wire: WireKind,
    body: &mut Value,
    response: &Value,
    results: &[(Call, String)],
) {
    match wire {
        WireKind::Anthropic => {
            let content = response
                .get("content")
                .cloned()
                .unwrap_or_else(|| json!([]));
            push(
                wire,
                body,
                json!({ "role": "assistant", "content": content }),
            );
            let blocks: Vec<Value> = results
                .iter()
                .map(|(call, text)| {
                    json!({ "type": "tool_result", "tool_use_id": call.id, "content": text })
                })
                .collect();
            push(wire, body, json!({ "role": "user", "content": blocks }));
        }
        WireKind::OpenAiChat => {
            if let Some(message) = response.pointer("/choices/0/message") {
                push(wire, body, message.clone());
            }
            for (call, text) in results {
                let item = json!({ "role": "tool", "tool_call_id": call.id, "content": text });
                push(wire, body, item);
            }
        }
        WireKind::OpenAiResponses => {
            for item in array(response.get("output")) {
                push(wire, body, item);
            }
            for (call, text) in results {
                let item =
                    json!({ "type": "function_call_output", "call_id": call.id, "output": text });
                push(wire, body, item);
            }
        }
        WireKind::Google => {
            if let Some(content) = google_content(response) {
                push(wire, body, content.clone());
            }
            let parts: Vec<Value> = results
                .iter()
                .map(|(call, text)| {
                    json!({ "functionResponse": { "name": call.name, "id": call.id, "response": { "result": text } } })
                })
                .collect();
            push(wire, body, json!({ "role": "user", "parts": parts }));
        }
    }
}

/// The response text, for the log.
pub fn text(wire: WireKind, response: &Value) -> String {
    let parts: Vec<String> = match wire {
        WireKind::Anthropic => array(response.get("content"))
            .iter()
            .filter_map(|block| {
                block
                    .get("text")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .collect(),
        WireKind::OpenAiChat => response
            .pointer("/choices/0/message/content")
            .and_then(Value::as_str)
            .map(str::to_string)
            .into_iter()
            .collect(),
        WireKind::OpenAiResponses => array(response.get("output"))
            .iter()
            .flat_map(|item| array(item.get("content")))
            .filter_map(|block| {
                block
                    .get("text")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .collect(),
        WireKind::Google => array(google_content(response).and_then(|c| c.get("parts")))
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str).map(str::to_string))
            .collect(),
    };
    parts.join("")
}

fn user_text(wire: WireKind, text: &str) -> Value {
    match wire {
        WireKind::Anthropic => {
            json!({ "role": "user", "content": [{ "type": "text", "text": text }] })
        }
        WireKind::OpenAiChat | WireKind::OpenAiResponses => {
            json!({ "role": "user", "content": text })
        }
        WireKind::Google => json!({ "role": "user", "parts": [{ "text": text }] }),
    }
}

fn push(wire: WireKind, body: &mut Value, item: Value) {
    let key = match wire {
        WireKind::Google => "contents",
        WireKind::OpenAiChat | WireKind::Anthropic => "messages",
        WireKind::OpenAiResponses => "input",
    };
    if let Some(items) = request_body_mut(body)
        .get_mut(key)
        .and_then(Value::as_array_mut)
    {
        items.push(item);
    }
}

/// The first candidate's content; Cloud Code wraps the response in `response`.
fn google_content(response: &Value) -> Option<&Value> {
    response
        .get("response")
        .unwrap_or(response)
        .pointer("/candidates/0/content")
}

fn array(value: Option<&Value>) -> Vec<Value> {
    value.and_then(Value::as_array).cloned().unwrap_or_default()
}

fn text_of(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(items: &[&str]) -> Vec<String> {
        items.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn the_final_answer_after_the_interval_starts_one_review() {
        let mut state = ReviewState::default();
        for _ in 0..3 {
            assert!(!state.round_ended(&names(&["mcp__el__read"]), 3));
        }
        assert!(state.round_ended(&[], 3));
        assert!(
            !state.round_ended(&[], 3),
            "a running review starts no second one"
        );
        state.finished();
        assert!(
            !state.round_ended(&[], 3),
            "the count restarted at the review"
        );
    }

    #[test]
    fn a_skill_write_resets_the_count_and_zero_turns_reviews_off() {
        let mut state = ReviewState::default();
        for _ in 0..4 {
            state.round_ended(&names(&["read"]), 3);
        }
        state.round_ended(&names(&["mcp__el__skill_manage"]), 3);
        assert!(!state.round_ended(&[], 3));
        let mut off = ReviewState::default();
        for _ in 0..9 {
            off.round_ended(&names(&["read"]), 0);
        }
        assert!(!off.round_ended(&[], 0));
    }

    #[test]
    fn only_a_request_with_the_skill_tool_is_reviewed() {
        let with = json!({ "tools": [{ "name": "mcp__el__skill_manage" }] });
        let wrapped = json!({ "request": { "tools": [{ "functionDeclarations": [{ "name": "skill_manage" }] }] } });
        assert!(offers_skill_tool(&with));
        assert!(offers_skill_tool(&wrapped));
        assert!(!offers_skill_tool(
            &json!({ "tools": [{ "name": "read" }] })
        ));
        assert!(!offers_skill_tool(&json!({ "messages": [] })));
    }

    #[test]
    fn the_fork_request_keeps_the_prefix_and_stops_streaming() {
        let body = json!({ "stream": true, "stream_options": {}, "messages": [{ "role": "user", "content": "hi" }] });
        let (url, fork) = fork_request(WireKind::OpenAiChat, "http://u/x", &body, "review");
        assert_eq!(url, "http://u/x");
        assert_eq!(fork["stream"], false);
        assert!(fork.get("stream_options").is_none());
        assert_eq!(fork["messages"][0], body["messages"][0]);
        assert_eq!(
            fork["messages"][1],
            json!({ "role": "user", "content": "review" })
        );

        let google = json!({ "request": { "contents": [] } });
        let (url, fork) = fork_request(
            WireKind::Google,
            "http://u/v1internal:streamGenerateContent?alt=sse",
            &google,
            "review",
        );
        assert_eq!(url, "http://u/v1internal:generateContent");
        assert_eq!(fork["request"]["contents"][0]["parts"][0]["text"], "review");
    }

    #[test]
    fn each_wire_reads_calls_and_appends_the_round_natively() {
        let cases = [
            (
                WireKind::Anthropic,
                json!({ "messages": [] }),
                json!({ "content": [{ "type": "text", "text": "ok" }, { "type": "tool_use", "id": "t1", "name": "read", "input": { "path": "a" } }] }),
                "tool_use_id",
            ),
            (
                WireKind::OpenAiChat,
                json!({ "messages": [] }),
                json!({ "choices": [{ "message": { "role": "assistant", "tool_calls": [{ "id": "t1", "function": { "name": "read", "arguments": "{\"path\":\"a\"}" } }] } }] }),
                "tool_call_id",
            ),
            (
                WireKind::OpenAiResponses,
                json!({ "input": [] }),
                json!({ "output": [{ "type": "function_call", "call_id": "t1", "name": "read", "arguments": "{\"path\":\"a\"}" }] }),
                "call_id",
            ),
            (
                WireKind::Google,
                json!({ "request": { "contents": [] } }),
                json!({ "response": { "candidates": [{ "content": { "role": "model", "parts": [{ "functionCall": { "id": "t1", "name": "read", "args": { "path": "a" } } }] } }] } }),
                "functionResponse",
            ),
        ];
        for (wire, mut body, response, marker) in cases {
            let found = calls(wire, &response);
            let expected = Call {
                id: "t1".into(),
                name: "read".into(),
                args: json!({ "path": "a" }),
            };
            assert_eq!(found, vec![expected.clone()], "{wire:?}");
            append_round(wire, &mut body, &response, &[(expected, "result".into())]);
            assert!(body.to_string().contains(marker), "{wire:?}");
            assert!(body.to_string().contains("result"), "{wire:?}");
        }
    }

    #[test]
    fn a_response_without_calls_ends_the_review() {
        let response = json!({ "choices": [{ "message": { "role": "assistant", "content": "Nothing to save." } }] });
        assert!(calls(WireKind::OpenAiChat, &response).is_empty());
        assert_eq!(text(WireKind::OpenAiChat, &response), "Nothing to save.");
    }
}
