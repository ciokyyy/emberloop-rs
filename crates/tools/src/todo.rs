//! `todo`: the plan for the turn as one ordered list the model rewrites whole.
//!
//! One list per Session, persisted next to `seen/` so a restart, a Session
//! switch or a second harness turn reads back exactly what the model last said.
//! The model replaces the list instead of patching it: an index-addressed
//! update would drift as soon as a tool call changes the plan, and a whole-list
//! write makes the rendered checklist and the stored file the same thing.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{ToolError, ToolResult, Toolbox};

#[derive(Deserialize)]
struct TodoArgs {
    todos: Vec<Value>,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Status {
    Pending,
    InProgress,
    Completed,
}

impl Status {
    fn parse(raw: Option<&Value>) -> ToolResult<Self> {
        let text = raw.and_then(Value::as_str).unwrap_or("pending").trim();
        match text.to_ascii_lowercase().as_str() {
            "" | "pending" | "todo" | "open" => Ok(Self::Pending),
            "in_progress" | "in-progress" | "doing" | "active" => Ok(Self::InProgress),
            "completed" | "complete" | "done" | "closed" => Ok(Self::Completed),
            other => Err(ToolError::msg(format!(
                "unknown todo status {other:?}; use pending, in_progress or completed"
            ))),
        }
    }

    fn mark(self) -> &'static str {
        match self {
            Self::Pending => "[ ]",
            Self::InProgress => "[~]",
            Self::Completed => "[x]",
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct Todo {
    content: String,
    status: Status,
}

/// The list lives beside `seen/` in the Session directory, unless the spill
/// directory has no parent (a bare temp dir in a test), where it lives inside.
fn store_path(toolbox: &Toolbox) -> PathBuf {
    toolbox
        .spill_dir()
        .parent()
        .map(|session| session.join("todos.json"))
        .unwrap_or_else(|| toolbox.spill_dir().join("todos.json"))
}

pub fn run(toolbox: &Toolbox, args: &Value) -> ToolResult<String> {
    let args: TodoArgs = serde_json::from_value(args.clone())?;
    let mut todos = Vec::with_capacity(args.todos.len());
    for raw in &args.todos {
        let content = raw
            .get("content")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .ok_or_else(|| ToolError::msg("every todo needs a non-empty `content`"))?;
        todos.push(Todo {
            content: content.to_string(),
            status: Status::parse(raw.get("status"))?,
        });
    }
    save(toolbox, &todos)?;
    Ok(render(&todos))
}

fn render(todos: &[Todo]) -> String {
    let count = |status: Status| todos.iter().filter(|todo| todo.status == status).count();
    if todos.is_empty() {
        return "No todos.".to_string();
    }
    let mut out = format!(
        "{} pending · {} in progress · {} completed\n",
        count(Status::Pending),
        count(Status::InProgress),
        count(Status::Completed)
    );
    for todo in todos {
        out.push_str(&format!("{} {}\n", todo.status.mark(), todo.content));
    }
    out
}

/// save rewrites the whole file through a temp name and a rename, so a reader
/// never sees half a list.
fn save(toolbox: &Toolbox, todos: &[Todo]) -> ToolResult<()> {
    let path = store_path(toolbox);
    if todos.is_empty() {
        match fs::remove_file(&path) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        }
    }
    let encoded = serde_json::to_vec_pretty(todos)?;
    let tmp = path.with_extension(format!("json.tmp.{}", std::process::id()));
    fs::write(&tmp, encoded)?;
    fs::rename(&tmp, &path)?;
    Ok(())
}

/// load reads the stored list back; the daemon shows it before the next turn.
pub fn load(toolbox: &Toolbox) -> Vec<(String, String)> {
    let Ok(bytes) = fs::read(store_path(toolbox)) else {
        return Vec::new();
    };
    let Ok(todos) = serde_json::from_slice::<Vec<Todo>>(&bytes) else {
        return Vec::new();
    };
    todos
        .into_iter()
        .map(|todo| {
            let status = match todo.status {
                Status::Pending => "pending",
                Status::InProgress => "in_progress",
                Status::Completed => "completed",
            };
            (todo.content, status.to_string())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn toolbox() -> (tempfile::TempDir, Toolbox) {
        let dir = tempfile::tempdir().unwrap();
        let spill = dir.path().join("session").join("spill");
        fs::create_dir_all(&spill).unwrap();
        let toolbox = Toolbox::new(crate::ToolConfig::new(dir.path(), &spill)).unwrap();
        (dir, toolbox)
    }

    #[test]
    fn renders_a_checklist_with_counts() {
        let (_dir, toolbox) = toolbox();
        let out = run(
            &toolbox,
            &json!({"todos": [
                {"content": "read the spec", "status": "completed"},
                {"content": "write the parser", "status": "in_progress"},
                {"content": "test it"}
            ]}),
        )
        .unwrap();
        assert_eq!(
            out,
            "1 pending · 1 in progress · 1 completed\n[x] read the spec\n[~] write the parser\n[ ] test it\n"
        );
    }

    #[test]
    fn keeps_the_list_for_the_next_turn() {
        let (_dir, toolbox) = toolbox();
        run(&toolbox, &json!({"todos": [{"content": "one"}]})).unwrap();
        assert_eq!(
            load(&toolbox),
            vec![("one".to_string(), "pending".to_string())]
        );
    }

    #[test]
    fn an_empty_list_clears_the_file() {
        let (_dir, toolbox) = toolbox();
        run(&toolbox, &json!({"todos": [{"content": "one"}]})).unwrap();
        let out = run(&toolbox, &json!({"todos": []})).unwrap();
        assert_eq!(out, "No todos.");
        assert!(load(&toolbox).is_empty());
        assert!(run(&toolbox, &json!({"todos": []})).is_ok());
    }

    #[test]
    fn rejects_a_blank_entry_and_an_unknown_status() {
        let (_dir, toolbox) = toolbox();
        assert!(run(&toolbox, &json!({"todos": [{"content": "  "}]})).is_err());
        assert!(run(
            &toolbox,
            &json!({"todos": [{"content": "x", "status": "later"}]})
        )
        .is_err());
    }
}
