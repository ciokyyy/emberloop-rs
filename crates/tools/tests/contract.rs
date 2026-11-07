#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Contract between the tool descriptions in the Prompt Store and the MCP input schemas.
//! Pure: no process, no sleep, no harness.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use rmcp::model::Tool;
use serde_json::{Map, Value};
use tools::{McpServer, ToolConfig, Toolbox};

/// The descriptions the daemon hands the server: every `tool.<name>.description` id.
fn prompt_descriptions() -> HashMap<String, String> {
    prompts::PROMPT_IDS
        .iter()
        .filter_map(|id| {
            let name = id.strip_prefix("tool.")?.strip_suffix(".description")?;
            Some((name.to_string(), prompts::default_text(id).to_string()))
        })
        .collect()
}

fn server() -> McpServer {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("project");
    std::fs::create_dir(&root).expect("root");
    let toolbox = Toolbox::new(ToolConfig::new(&root, dir.path().join("spill"))).expect("toolbox");
    McpServer::new(Arc::new(toolbox))
}

fn advertised() -> Vec<Tool> {
    server().with_descriptions(prompt_descriptions()).tools()
}

fn schema(tool: &Tool) -> Value {
    Value::Object(Map::clone(&tool.input_schema))
}

fn description(tool: &Tool) -> String {
    tool.description.as_deref().unwrap_or_default().to_string()
}

/// Every `(name, schema)` property at any depth of a schema.
fn properties(value: &Value, out: &mut Vec<(String, Value)>) {
    match value {
        Value::Object(map) => {
            if let Some(Value::Object(props)) = map.get("properties") {
                for (name, schema) in props {
                    out.push((name.clone(), schema.clone()));
                    properties(schema, out);
                }
            }
            for (key, child) in map {
                if key != "properties" {
                    properties(child, out);
                }
            }
        }
        Value::Array(items) => items.iter().for_each(|item| properties(item, out)),
        _ => {}
    }
}

/// Every enum value at any depth of a schema.
fn enum_values(value: &Value, out: &mut BTreeSet<String>) {
    match value {
        Value::Object(map) => {
            if let Some(Value::Array(values)) = map.get("enum") {
                out.extend(values.iter().filter_map(Value::as_str).map(str::to_string));
            }
            map.values().for_each(|child| enum_values(child, out));
        }
        Value::Array(items) => items.iter().for_each(|item| enum_values(item, out)),
        _ => {}
    }
}

/// Code spans (text between backticks) outside fenced code blocks.
fn code_spans(text: &str) -> Vec<String> {
    let mut spans = Vec::new();
    let mut in_fence = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        for (index, part) in line.split('`').enumerate() {
            if index % 2 == 1 {
                spans.push(part.to_string());
            }
        }
    }
    spans
}

fn is_parameter_word(span: &str) -> bool {
    !span.is_empty() && span.chars().all(|c| c.is_ascii_lowercase() || c == '_')
}

#[test]
fn every_advertised_tool_has_a_prompt_store_description() {
    for tool in advertised() {
        let id = format!("tool.{}.description", tool.name);
        assert!(
            prompts::PROMPT_IDS.contains(&id.as_str()),
            "{} has no prompt id {id}",
            tool.name
        );
        assert!(
            !description(&tool).trim().is_empty(),
            "{} has an empty description",
            tool.name
        );
    }
}

#[test]
fn backticked_words_in_tool_text_are_real_parameters_values_or_tools() {
    let tools = advertised();
    let tool_names: BTreeSet<String> = tools.iter().map(|t| t.name.to_string()).collect();
    let mut failures = Vec::new();
    for tool in &tools {
        let schema = schema(tool);
        let mut names: BTreeSet<String> = tool_names.clone();
        let mut props = Vec::new();
        properties(&schema, &mut props);
        names.extend(props.into_iter().map(|(name, _)| name));
        enum_values(&schema, &mut names);
        for span in code_spans(&description(tool)) {
            if is_parameter_word(&span) && !names.contains(&span) {
                failures.push(format!(
                    "{}: `{span}` is not a parameter, value or tool",
                    tool.name
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn every_top_level_parameter_is_named_in_its_tool_text() {
    let mut failures = Vec::new();
    for tool in advertised() {
        let schema = schema(&tool);
        let text = description(&tool);
        for name in schema
            .get("properties")
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|props| props.keys())
        {
            if !code_spans(&text).iter().any(|span| span == name) {
                failures.push(format!(
                    "{}: parameter `{name}` is not named in its text",
                    tool.name
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn every_timeout_states_its_unit_in_seconds() {
    let mut failures = Vec::new();
    for tool in advertised() {
        let mut props = Vec::new();
        properties(&schema(&tool), &mut props);
        for (name, prop) in props {
            if name != "timeout" {
                continue;
            }
            let described = prop
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if !described.to_ascii_lowercase().contains("second") {
                failures.push(format!(
                    "{}: timeout has no unit in its description",
                    tool.name
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn a_bare_server_advertises_the_prompt_store_defaults() {
    for tool in server().tools() {
        let id = format!("tool.{}.description", tool.name);
        assert_eq!(
            description(&tool),
            prompts::default_text(&id),
            "{} must not carry its own description text",
            tool.name
        );
    }
}

#[test]
fn read_path_names_every_form_read_accepts() {
    let tools = advertised();
    let read = tools.iter().find(|tool| tool.name == "read").expect("read");
    let path = schema(read)["properties"]["path"]["description"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    for form in ["directory", "URL", "ctx:<handle>"] {
        assert!(path.contains(form), "read path text lacks {form}: {path}");
    }
}
