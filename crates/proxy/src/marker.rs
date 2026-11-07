//! The marker lines the proxy writes into conversation text: the summary a fold leaves where its
//! turns stood, and the `📦`/`❌ [acp]` line a proxy tool result carries. Every builder and every
//! prefix match goes through here, so the wire text has one home.

use std::fmt::Display;

/// First line of a successful proxy tool result, followed by the tool name.
pub const TOOL_OK: &str = "📦 [acp]";
/// First line of a failed proxy tool result, followed by the tool name.
pub const TOOL_ERR: &str = "❌ [acp]";
const SUMMARY_PREFIX: &str = "📦 [acp] Compressed turns ";

/// The text of the summary turn a fold stores. `last` is the last folded turn, inclusive.
pub fn summary(
    first: usize,
    last: usize,
    block_id: &str,
    topic: Option<&str>,
    body: &str,
) -> String {
    let topic = topic.map(|topic| format!(" · {topic}")).unwrap_or_default();
    format!("{SUMMARY_PREFIX}{first}–{last} into block {block_id}{topic}\n\n{body}")
}

/// Whether `text` is a summary turn a fold stored.
pub fn is_summary(text: &str) -> bool {
    text.starts_with(SUMMARY_PREFIX)
}

/// Whether `line` or `text` starts with a proxy tool marker, successful or failed. A model line
/// that does is forged and stripped.
pub fn is_tool_text(text: &str) -> bool {
    text.starts_with(TOOL_OK) || text.starts_with(TOOL_ERR)
}

/// The text of a successful proxy tool result: its marker line, then the tool's output.
pub fn tool_result(name: &str, output: &str) -> String {
    format!("{TOOL_OK} {name}\n{output}")
}

/// The text of a failed proxy tool result.
pub fn tool_failure(name: &str, error: impl Display) -> String {
    format!("{TOOL_ERR} {name}\nerror: {error}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_names_the_turns_block_topic_and_body() {
        assert_eq!(
            summary(1, 4, "b1", None, "summary of 1..5"),
            "📦 [acp] Compressed turns 1–4 into block b1\n\nsummary of 1..5"
        );
        assert_eq!(
            summary(1, 4, "b1", Some("billing"), "summary of 1..5"),
            "📦 [acp] Compressed turns 1–4 into block b1 · billing\n\nsummary of 1..5"
        );
    }

    #[test]
    fn only_a_built_summary_is_a_summary_marker() {
        assert!(is_summary(&summary(0, 3, "b1", None, "x")));
        assert!(!is_summary("📦 [acp] compress\nanswer"));
        assert!(!is_summary("Compressed turns 0–3"));
    }

    #[test]
    fn tool_markers_are_recognised_whatever_the_outcome() {
        assert!(is_tool_text("📦 [acp] compress"));
        assert!(is_tool_text("❌ [acp] decompress"));
        assert!(!is_tool_text("plain answer"));
    }

    #[test]
    fn tool_results_and_failures_carry_the_tool_name_on_the_marker_line() {
        assert_eq!(tool_result("compress", "done"), "📦 [acp] compress\ndone");
        assert_eq!(
            tool_failure("compress", "bad range"),
            "❌ [acp] compress\nerror: bad range"
        );
    }
}
