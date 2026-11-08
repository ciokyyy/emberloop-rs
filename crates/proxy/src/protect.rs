//! The one notion of protected content: material no fold may select, replace by a marker, hide,
//! or count as foldable. Two kinds are protected. System and developer items (the chat, Responses
//! and Google wires' system roles, and Anthropic's `system` role should one appear in messages)
//! are never conversation turns: `wire.rs` keeps them out of the core turns and writes them back
//! byte-exact. A user turn with a text part carrying a `<system-reminder>` tag is protected as a
//! whole turn: Claude Code puts its CLAUDE.md, environment and skill text in those. Every fold path
//! asks `is_protected` of a turn, so no wire or harness names the rule itself.

use serde_json::Value;

use crate::turnkeys;

/// The tag Claude Code wraps harness context in, inside a user message's text.
const SYSTEM_REMINDER: &str = "<system-reminder>";

/// Whether a wire item carries the system or developer role.
pub fn is_system_item(item: &Value) -> bool {
    item.get("role")
        .and_then(Value::as_str)
        .is_some_and(|role| matches!(role, "system" | "developer"))
}

/// Whether a core turn is protected: its wire message is system material, or it is a user turn
/// with a text part that carries a system reminder. Tool results are not text parts, so their
/// content never makes a turn protected.
pub fn is_protected(turn: &Value) -> bool {
    turn.get(turnkeys::RAW).is_some_and(is_system_item)
        || (turn.get("role").and_then(Value::as_str) == Some("user") && has_reminder(turn))
}

fn has_reminder(turn: &Value) -> bool {
    turn.get("parts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|part| part.get("text")?.as_str())
        .any(|text| text.contains(SYSTEM_REMINDER))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn core(role: &str, text: &str) -> Value {
        json!({"role": role, "parts": [{"text": text}]})
    }

    #[test]
    fn a_user_turn_with_a_system_reminder_part_is_protected() {
        let turn = json!({"role": "user", "parts": [
            {"text": "<system-reminder>CLAUDE.md</system-reminder>"},
            {"text": "fix the bug"}
        ]});
        assert!(is_protected(&turn));
        assert!(is_protected(&core("user", "a\n<system-reminder>b")));
    }

    #[test]
    fn plain_user_and_model_text_is_not_protected() {
        assert!(!is_protected(&core("user", "fix the bug")));
        assert!(!is_protected(&core(
            "model",
            "<system-reminder>quoted</system-reminder>"
        )));
    }

    #[test]
    fn a_tool_result_is_not_protected_by_text_inside_it() {
        let turn = json!({"role": "user", "parts": [{"functionResponse": {
            "id": "c1", "name": "read", "response": {"result": "<system-reminder>file text</system-reminder>"}
        }}]});
        assert!(!is_protected(&turn));
    }

    #[test]
    fn a_turn_whose_wire_message_has_the_system_or_developer_role_is_protected() {
        for role in ["system", "developer"] {
            let turn = json!({"role": "user", "parts": [{"text": "policy"}], "_raw": {"role": role, "content": "policy"}});
            assert!(is_protected(&turn), "{role}");
        }
        let turn = json!({"role": "user", "parts": [{"text": "x"}], "_raw": {"role": "user"}});
        assert!(!is_protected(&turn));
    }

    #[test]
    fn system_items_are_the_system_and_developer_roles_only() {
        assert!(is_system_item(&json!({"role": "system", "content": "x"})));
        assert!(is_system_item(
            &json!({"type": "message", "role": "developer", "content": []})
        ));
        assert!(!is_system_item(&json!({"role": "user", "content": "x"})));
        assert!(!is_system_item(
            &json!({"type": "function_call", "call_id": "c"})
        ));
    }
}
