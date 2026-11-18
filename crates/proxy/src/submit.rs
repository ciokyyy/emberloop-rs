//! An accepted `submit` ends a subagent run: the report already reached the parent, so the tool
//! result gets no model round. The proxy answers the wire with an empty turn instead, and the
//! parent's next steer resumes the child. A rejected submit is an ordinary tool result.

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::wire::{extract_core_contents, WireKind};

/// The last turn is a tool result for `submit` that the api accepted.
///
/// The harness calls the tool through a bridge that sends its own name and the real one in
/// `args.ToolName`, and a newer harness names it `mcp__el__submit`, so both count as `submit`.
pub fn accepted(wire: WireKind, request: &Value) -> bool {
    let Some(contents) = extract_core_contents(wire, request) else {
        return false;
    };
    let parts = contents
        .last()
        .and_then(|turn| turn.get("parts"))
        .and_then(Value::as_array);
    let calls: HashMap<&str, &Value> = contents
        .iter()
        .filter_map(|turn| turn.get("parts").and_then(Value::as_array))
        .flatten()
        .filter_map(|part| part.get("functionCall"))
        .filter_map(|call| Some((call.get("id")?.as_str()?, call)))
        .collect();
    parts.is_some_and(|parts| {
        parts
            .iter()
            .filter_map(|part| part.get("functionResponse"))
            .any(|result| {
                let call = result
                    .get("id")
                    .and_then(Value::as_str)
                    .and_then(|id| calls.get(id).copied());
                is_submit(result, call) && is_accepted(result)
            })
    })
}

fn is_submit(result: &Value, call: Option<&Value>) -> bool {
    let named = [result.get("name"), call.and_then(|call| call.get("name"))]
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .any(is_submit_name);
    let bridged = call
        .and_then(|call| call.get("args"))
        .and_then(|args| args.get("ToolName"))
        .and_then(Value::as_str)
        == Some("submit");
    named || bridged
}

/// `submit`, `mcp__el__submit` and `el_submit` all end in the tool's own name.
fn is_submit_name(name: &str) -> bool {
    name.rsplit(['_', '-']).next() == Some("submit")
}

/// The api answers an accepted call with `{"id":"submit_...","status":"done"|"blocked"}`; a
/// rejection is the plain text of a 400 and carries no such key.
fn is_accepted(result: &Value) -> bool {
    let response = result.get("response").map(text_of).unwrap_or_default();
    let squeezed: String = response.chars().filter(|c| !c.is_whitespace()).collect();
    ["\"status\":\"done\"", "\"status\":\"blocked\""]
        .iter()
        .any(|key| squeezed.contains(key))
}

/// Every string a result object holds, unescaped however deep it is nested: the api's JSON arrives
/// as the text of the harness's own `output` field.
fn text_of(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(items) => items.iter().map(text_of).collect::<Vec<_>>().join("\n"),
        Value::Object(map) => map.values().map(text_of).collect::<Vec<_>>().join("\n"),
        other => other.to_string(),
    }
}

/// An empty assistant turn on the request's wire, in the form the harness expects: a stream that
/// ends with the wire's terminal frames, or the buffered answer.
pub fn answer(
    wire: WireKind,
    request: &Value,
    path: &str,
    query: Option<&str>,
) -> (&'static str, String) {
    let streaming = is_streaming(wire, request, path, query);
    let body = match wire {
        WireKind::Google => google(request, streaming),
        WireKind::OpenAiChat => chat(request, streaming),
        WireKind::Anthropic => anthropic(request, streaming),
        WireKind::OpenAiResponses => responses(streaming),
    };
    let content_type = if streaming {
        "text/event-stream"
    } else {
        "application/json"
    };
    (content_type, body)
}

fn is_streaming(wire: WireKind, request: &Value, path: &str, query: Option<&str>) -> bool {
    if wire != WireKind::Google {
        return request
            .get("stream")
            .and_then(Value::as_bool)
            .unwrap_or(false);
    }
    path.contains(":streamGenerateContent") || query.is_some_and(|query| query.contains("alt=sse"))
}

fn model_of(request: &Value) -> &str {
    request.get("model").and_then(Value::as_str).unwrap_or("")
}

fn google(request: &Value, streaming: bool) -> String {
    let candidate = json!({
        "content": { "role": "model", "parts": [] },
        "finishReason": "STOP",
        "index": 0
    });
    let mut answer = json!({ "candidates": [candidate] });
    // Cloud Code nests the payload, and its client reads it there.
    if request.get("request").is_some() {
        answer = json!({ "response": answer });
    }
    if streaming {
        format!("data: {answer}\n\n")
    } else {
        answer.to_string()
    }
}

fn chat(request: &Value, streaming: bool) -> String {
    let model = model_of(request);
    let chunk = |finish: Value| {
        json!({
            "id": "chatcmpl-submitted",
            "object": "chat.completion.chunk",
            "created": 0,
            "model": model,
            "choices": [{ "index": 0, "delta": { "role": "assistant" }, "finish_reason": finish }]
        })
    };
    if !streaming {
        return json!({
            "id": "chatcmpl-submitted",
            "object": "chat.completion",
            "created": 0,
            "model": model,
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": "" },
                "finish_reason": "stop"
            }],
            "usage": { "prompt_tokens": 0, "completion_tokens": 0, "total_tokens": 0 }
        })
        .to_string();
    }
    format!(
        "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
        chunk(Value::Null),
        chunk(json!("stop"))
    )
}

fn anthropic(request: &Value, streaming: bool) -> String {
    let message = json!({
        "id": "msg_submitted",
        "type": "message",
        "role": "assistant",
        "model": model_of(request),
        "content": [],
        "stop_reason": if streaming { Value::Null } else { json!("end_turn") },
        "stop_sequence": null,
        "usage": { "input_tokens": 0, "output_tokens": 0 }
    });
    if !streaming {
        return message.to_string();
    }
    let start = json!({ "type": "message_start", "message": message });
    let delta = json!({
        "type": "message_delta",
        "delta": { "stop_reason": "end_turn", "stop_sequence": null },
        "usage": { "output_tokens": 0 }
    });
    let stop = json!({ "type": "message_stop" });
    format!(
        "event: message_start\ndata: {start}\n\n\
         event: message_delta\ndata: {delta}\n\n\
         event: message_stop\ndata: {stop}\n\n"
    )
}

fn responses(streaming: bool) -> String {
    let response = json!({
        "id": "resp_submitted",
        "object": "response",
        "status": "completed",
        "output": []
    });
    if !streaming {
        return response.to_string();
    }
    let completed = json!({ "type": "response.completed", "response": response });
    format!("event: response.completed\ndata: {completed}\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn submit_turns(result: Value, name: &str) -> Value {
        json!({
            "contents": [
                { "role": "user", "parts": [{ "text": "review it" }] },
                { "role": "model", "parts": [{ "functionCall": {
                    "id": "call_1", "name": name, "args": {}
                }}]},
                { "role": "user", "parts": [{ "functionResponse": {
                    "id": "call_1", "name": name, "response": result
                }}]}
            ]
        })
    }

    fn accepted_result() -> Value {
        json!({ "result": "{\"id\":\"submit_ab\",\"status\":\"done\"}" })
    }

    #[test]
    fn a_result_named_by_the_tool_ends_the_run() {
        for name in ["submit", "mcp__el__submit", "el_submit"] {
            assert!(accepted(
                WireKind::Google,
                &submit_turns(accepted_result(), name)
            ));
        }
    }

    #[test]
    fn a_bridged_call_is_recognized_by_its_tool_name_argument() {
        let request = json!({ "contents": [
            { "role": "model", "parts": [{ "functionCall": {
                "id": "call_1", "name": "call_mcp_tool", "args": { "ToolName": "submit" }
            }}]},
            { "role": "user", "parts": [{ "functionResponse": {
                "id": "call_1", "name": "call_mcp_tool", "response": accepted_result()
            }}]}
        ]});
        assert!(accepted(WireKind::Google, &request));
        let other = json!({ "contents": [
            { "role": "model", "parts": [{ "functionCall": {
                "id": "call_1", "name": "call_mcp_tool", "args": { "ToolName": "steer" }
            }}]},
            { "role": "user", "parts": [{ "functionResponse": {
                "id": "call_1", "name": "call_mcp_tool", "response": accepted_result()
            }}]}
        ]});
        assert!(!accepted(WireKind::Google, &other));
    }

    #[test]
    fn a_rejected_or_other_tool_result_keeps_the_round() {
        let rejected = json!({ "result": "400 Bad Request: status must be done or blocked" });
        assert!(!accepted(
            WireKind::Google,
            &submit_turns(rejected, "mcp__el__submit")
        ));
        assert!(!accepted(
            WireKind::Google,
            &submit_turns(accepted_result(), "bash")
        ));
    }

    #[test]
    fn every_wire_closes_its_own_way() {
        let request = json!({ "model": "m", "stream": true });
        let (content_type, body) = answer(WireKind::OpenAiChat, &request, "x", None);
        assert_eq!(content_type, "text/event-stream");
        assert!(body.contains("\"finish_reason\":\"stop\""));
        let (content_type, body) = answer(WireKind::OpenAiResponses, &request, "x", None);
        assert_eq!(content_type, "text/event-stream");
        assert!(body.contains("\"type\":\"response.completed\""));
        let cloud_code = json!({ "request": { "contents": [] } });
        let (_, body) = answer(
            WireKind::Google,
            &cloud_code,
            "v1internal:generateContent",
            None,
        );
        assert!(body.starts_with("{\"response\":"));
    }
}
