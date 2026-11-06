#![allow(clippy::unwrap_used, clippy::expect_used)]
use proxy::{context::ContextBreakdown, wire::WireKind};
use serde_json::json;

#[test]
fn chat_counts_system_tools_summaries_and_images_without_base64() {
    let request = json!({"messages":[
        {"role":"system","content":"policy"},
        {"role":"developer","content":"instructions"},
        {"role":"user","content":[{"type":"text","text":"hello"}, {"type":"image_url","image_url":{"url":format!("data:image/png;base64,{}", "A".repeat(100000))}}]},
        {"role":"assistant","tool_calls":[{"id":"c1","function":{"name":"read","arguments":"{}"}}]},
        {"role":"tool","content":"file contents","tool_call_id":"c1"},
        {"role":"user","content":"📦 [acp] Compressed turns 0–3 into block b1\n\nsummary"}
    ],"tools":[{"type":"function","function":{"name":"read","parameters":{"type":"object"}}}]});
    let b = ContextBreakdown::from_request(WireKind::OpenAiChat, &request);
    assert!(b.system > 0 && b.tool_definitions > 0 && b.tool_calls > 0 && b.tool_results > 0);
    assert!(b.compression > 0 && b.conversation > 0);
    assert_eq!(b.image_count, 1);
    assert!(b.estimated_tokens < 200);
    assert_eq!(
        b.estimated_tokens,
        b.system
            + b.tool_definitions
            + b.tool_calls
            + b.tool_results
            + b.compression
            + b.conversation
    );
}

#[test]
fn google_wrapper_and_anthropic_blocks_are_classified() {
    for (wire, request) in [
        (
            WireKind::Google,
            json!({"request":{"systemInstruction":{"parts":[{"text":"policy"}]},"tools":[{"functionDeclarations":[{"name":"read"}]}],"contents":[{"role":"model","parts":[{"functionCall":{"name":"read","args":{}}}]},{"role":"user","parts":[{"functionResponse":{"name":"read","response":{"result":"bytes"}}},{"inlineData":{"mimeType":"image/png","data":"AAAA"}}]}]}}),
        ),
        (
            WireKind::Anthropic,
            json!({"system":[{"type":"text","text":"policy"}],"tools":[{"name":"read"}],"messages":[{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"read","input":{}}]},{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":[{"type":"text","text":"result"},{"type":"image","source":{"type":"base64","data":"AAAA"}}]}]}]}),
        ),
    ] {
        let b = ContextBreakdown::from_request(wire, &request);
        assert!(
            b.system > 0 && b.tool_definitions > 0 && b.tool_calls > 0 && b.tool_results > 0,
            "{b:?}"
        );
        assert_eq!(b.image_count, 1);
    }
}

#[test]
fn responses_instructions_and_function_outputs_are_not_conversation() {
    let b = ContextBreakdown::from_request(
        WireKind::OpenAiResponses,
        &json!({
        "instructions":"policy", "input":[
            {"type":"function_call","name":"read","arguments":"{}","call_id":"1"},
            {"type":"function_call_output","call_id":"1","output":"result"},
            {"role":"user","content":[{"type":"input_image","image_url":"data:image/png;base64,AAAA"}]}
        ]}),
    );
    assert!(b.system > 0 && b.tool_calls > 0 && b.tool_results > 0);
    assert_eq!(b.conversation, 0);
    assert_eq!(b.image_count, 1);
}

#[test]
fn structured_google_tool_results_count_fields_not_just_a_result_key() {
    let b = ContextBreakdown::from_request(
        WireKind::Google,
        &json!({"contents":[
            {"role":"user","parts":[{"functionResponse":{"name":"list", "response":{"files":["file1.rs","file2.rs"],"count":2}}}]}
        ]}),
    );
    assert!(b.tool_results > 0);
    assert_eq!(b.conversation, 0);
}
