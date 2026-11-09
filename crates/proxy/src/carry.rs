//! Carries a Session's history into a new harness conversation (a harness or model switch).
//! The history is core turns, so any wire can write it; only what the new wire cannot take is cut.

use serde_json::Value;

use crate::turnkeys;

/// Core turns without the wire copy (`_raw`), which would replay the old wire's bytes, and without
/// `opaque` parts (thinking, signatures, unknown blocks) and empty text. Turns left empty drop;
/// neighbours with one role merge, because several wires refuse two turns of one role in a row.
pub fn sanitize(turns: Vec<Value>) -> Vec<Value> {
    let mut out = Vec::new();
    for mut turn in turns {
        if let Some(object) = turn.as_object_mut() {
            object.remove(turnkeys::RAW);
        }
        let parts: Vec<Value> = turn["parts"]
            .as_array()
            .map(|parts| {
                parts
                    .iter()
                    .filter(|p| p.get("opaque").is_none() && p.get("text") != Some(&"".into()))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        if parts.is_empty() {
            continue;
        }
        turn["parts"] = Value::Array(parts);
        push_merging(&mut out, turn);
    }
    out
}

/// The carried history followed by the new conversation's turns, merged at the seam.
pub fn join(mut carried: Vec<Value>, incoming: Vec<Value>) -> Vec<Value> {
    let mut incoming = incoming.into_iter();
    if let Some(first) = incoming.next() {
        push_merging(&mut carried, first);
    }
    carried.extend(incoming);
    carried
}

fn push_merging(out: &mut Vec<Value>, turn: Value) {
    match out.last_mut() {
        Some(last) if last["role"] == turn["role"] => {
            if let (Some(into), Some(from)) =
                (last["parts"].as_array_mut(), turn["parts"].as_array())
            {
                into.extend(from.iter().cloned());
            }
        }
        _ => out.push(turn),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sanitize_cuts_wire_leftovers_and_merges_roles() {
        let turns = vec![
            json!({"role": "user", "parts": [{"text": "a"}], "_raw": {"x": 1}}),
            json!({"role": "model", "parts": [{"opaque": {"type": "thinking"}}, {"text": ""}]}),
            json!({"role": "user", "parts": [{"text": "b"}]}),
        ];
        assert_eq!(
            sanitize(turns),
            vec![json!({"role": "user", "parts": [{"text": "a"}, {"text": "b"}]})]
        );
    }

    #[test]
    fn join_merges_the_seam() {
        let joined = join(
            vec![json!({"role": "user", "parts": [{"text": "a"}]})],
            vec![json!({"role": "user", "parts": [{"text": "b"}]})],
        );
        assert_eq!(joined.len(), 1);
    }
}
