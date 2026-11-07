//! The one token estimate the proxy uses for conversation text. It is the rule of the
//! billion-context reference (`defaultCountTokens` in its `kernel/src/tokenize.ts`): every CJK
//! character counts one token, and the other characters of a text count `ceil(n / 4)`. The
//! count is over characters, not bytes. Nudge timing, preflight folds, the tokens a fold saved and
//! the context breakdown all read this, so they agree with each other and with the reference.

use serde_json::Value;

/// Tokens of one text: CJK characters 1:1, the rest `ceil(n / 4)` for `n` non-CJK characters.
pub fn text_tokens(text: &str) -> u64 {
    let total = text.chars().count();
    let cjk = text.chars().filter(|c| is_cjk(*c)).count();
    let rest = total - cjk;
    u64::try_from(cjk + rest.div_ceil(4)).unwrap_or(u64::MAX)
}

/// Tokens of a structured value, counted as its serialised JSON text.
pub fn json_tokens(value: &Value) -> u64 {
    text_tokens(&value.to_string())
}

/// Tokens of one conversation turn in the core format: the sum of its text-bearing strings. Message
/// text counts as is, a function call counts its name and its arguments, a function result counts
/// every string in its response. Images and other parts count nothing.
pub fn turn_tokens(turn: &Value) -> u64 {
    turn.get("parts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(part_tokens)
        .sum()
}

/// Tokens of a list of turns: the sum of [`turn_tokens`].
pub fn turns_tokens(turns: &[Value]) -> u64 {
    turns.iter().map(turn_tokens).sum()
}

fn part_tokens(part: &Value) -> u64 {
    if let Some(text) = part.get("text").and_then(Value::as_str) {
        return text_tokens(text);
    }
    if let Some(call) = part.get("functionCall") {
        let name = call
            .get("name")
            .and_then(Value::as_str)
            .map_or(0, text_tokens);
        let args = call.get("args").map_or(0, json_text_tokens);
        return name + args;
    }
    if let Some(result) = part.get("functionResponse") {
        let mut strings = Vec::new();
        collect_strings(result.get("response").unwrap_or(&Value::Null), &mut strings);
        return strings.iter().map(|text| text_tokens(text)).sum();
    }
    0
}

/// Arguments are read as the text the model wrote: a string argument is its own text.
fn json_text_tokens(value: &Value) -> u64 {
    match value {
        Value::String(text) => text_tokens(text),
        other => json_tokens(other),
    }
}

fn collect_strings<'a>(value: &'a Value, out: &mut Vec<&'a str>) {
    match value {
        Value::String(text) => out.push(text),
        Value::Array(items) => items.iter().for_each(|item| collect_strings(item, out)),
        Value::Object(map) => map.values().for_each(|item| collect_strings(item, out)),
        _ => {}
    }
}

/// The CJK ranges the reference counts one token each: CJK ideographs, Hiragana and Katakana, and
/// Hangul syllables.
fn is_cjk(c: char) -> bool {
    matches!(c,
        '\u{4e00}'..='\u{9fff}' | '\u{3040}'..='\u{30ff}' | '\u{ac00}'..='\u{d7af}')
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ascii_text_is_a_quarter_of_its_length_rounded_up() {
        assert_eq!(text_tokens(""), 0);
        assert_eq!(text_tokens("abcd"), 1);
        assert_eq!(text_tokens("abcde"), 2);
    }

    #[test]
    fn cjk_characters_count_one_token_each() {
        assert_eq!(text_tokens("你好"), 2);
        // Four CJK characters, then two ASCII ones: 4 + ceil(2 / 4).
        assert_eq!(text_tokens("你好世界ab"), 5);
        assert_eq!(text_tokens("ひらがなカタカナ한국"), 10);
    }

    #[test]
    fn text_is_counted_in_characters_not_bytes() {
        // Five characters of two bytes each: ten bytes, but five characters, so two tokens.
        assert_eq!(text_tokens("ééééé"), 2);
    }

    #[test]
    fn a_turn_sums_its_text_call_and_result_parts() {
        let turn = json!({
            "role": "model",
            "parts": [
                {"text": "abcde"},
                {"functionCall": {"name": "read", "args": {"path": "a"}, "id": "c1"}},
                {"functionResponse": {"name": "read", "id": "c1", "response": {"result": "abcdefgh"}}}
            ]
        });
        // "abcde" is 2, the name "read" is 1, the arguments `{"path":"a"}` are 12 characters
        // (3), the result "abcdefgh" is 2. The id and the tool name of a result are not text.
        assert_eq!(turn_tokens(&turn), 2 + 1 + 3 + 2);
    }

    #[test]
    fn a_string_argument_counts_as_its_own_text() {
        let turn = json!({"parts": [{"functionCall": {"name": "x", "args": "abcdefgh"}}]});
        assert_eq!(turn_tokens(&turn), 1 + 2);
    }

    #[test]
    fn an_empty_turn_and_an_image_count_nothing() {
        assert_eq!(turn_tokens(&json!({"role": "user"})), 0);
        assert_eq!(turn_tokens(&json!({"parts": []})), 0);
        assert_eq!(
            turn_tokens(
                &json!({"parts": [{"inlineData": {"mimeType": "image/png", "data": "AAAA"}}]})
            ),
            0
        );
    }

    #[test]
    fn a_list_of_turns_sums_its_turns() {
        let turns = [
            json!({"parts": [{"text": "abcde"}]}),
            json!({"parts": [{"text": "你"}]}),
        ];
        assert_eq!(turns_tokens(&turns), 3);
    }
}
