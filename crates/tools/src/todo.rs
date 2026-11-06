//! `todo`: the plan as one ordered list, addressed by stable ids.
//!
//! One list per Session, persisted next to `seen/` so a restart, a Session
//! switch or a second harness turn reads back exactly what the model last said.
//! Ids never change and are never reused, so `update` and `remove` can name an
//! item without the plan drifting under the model.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{ToolError, ToolResult, Toolbox};

#[derive(Deserialize)]
struct TodoArgs {
    action: String,
    #[serde(default)]
    items: Vec<Item>,
    #[serde(default)]
    updates: Vec<Update>,
    #[serde(default)]
    ids: Vec<String>,
    after: Option<String>,
}

#[derive(Deserialize)]
struct Item {
    content: String,
    status: Option<Value>,
}

#[derive(Deserialize)]
struct Update {
    id: String,
    status: Option<Value>,
    content: Option<String>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pending,
    InProgress,
    Completed,
    Cancelled,
}

impl Status {
    fn parse(raw: Option<&Value>) -> ToolResult<Self> {
        let text = raw.and_then(Value::as_str).unwrap_or("pending").trim();
        match text.to_ascii_lowercase().as_str() {
            "" | "pending" | "todo" | "open" => Ok(Self::Pending),
            "in_progress" | "in-progress" | "doing" | "active" => Ok(Self::InProgress),
            "completed" | "complete" | "done" | "closed" => Ok(Self::Completed),
            "cancelled" | "canceled" | "dropped" => Ok(Self::Cancelled),
            other => Err(ToolError::msg(format!(
                "unknown todo status {other:?}; use pending, in_progress, completed or cancelled"
            ))),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
        }
    }

    fn mark(self) -> &'static str {
        match self {
            Self::Pending => "[ ]",
            Self::InProgress => "[~]",
            Self::Completed => "[x]",
            Self::Cancelled => "[–]",
        }
    }

    /// Open means the item still asks for work, so it still triggers reminders.
    pub fn open(self) -> bool {
        matches!(self, Self::Pending | Self::InProgress)
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Todo {
    #[serde(default)]
    pub id: String,
    pub content: String,
    pub status: Status,
}

#[derive(Default, Serialize, Deserialize)]
struct Plan {
    #[serde(default)]
    next: u64,
    #[serde(default)]
    items: Vec<Todo>,
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
    let mut plan = load_plan(toolbox);
    let action = args.action.trim().to_ascii_lowercase();
    let out = match action.as_str() {
        "read" => render_full(&plan.items),
        "set" => {
            let mut items = Vec::with_capacity(args.items.len());
            for item in &args.items {
                items.push(Todo {
                    id: format!("t{}", items.len() + 1),
                    content: content(&item.content)?,
                    status: Status::parse(item.status.as_ref())?,
                });
            }
            validate(&items)?;
            plan.next = items.len() as u64 + 1;
            plan.items = items;
            render_full(&plan.items)
        }
        "add" => {
            if args.items.is_empty() {
                return Err(ToolError::msg("add needs at least one entry in `items`"));
            }
            let mut fresh = Vec::with_capacity(args.items.len());
            for item in &args.items {
                fresh.push(Todo {
                    id: format!("t{}", plan.next),
                    content: content(&item.content)?,
                    status: Status::parse(item.status.as_ref())?,
                });
                plan.next += 1;
            }
            let at = match &args.after {
                Some(after) => plan
                    .items
                    .iter()
                    .position(|todo| &todo.id == after)
                    .map(|index| index + 1)
                    .ok_or_else(|| unknown(after))?,
                None => plan.items.len(),
            };
            let ids: Vec<String> = fresh.iter().map(|todo| todo.id.clone()).collect();
            plan.items.splice(at..at, fresh);
            validate(&plan.items)?;
            format!("{} added · {}", ids.join(", "), left(&plan.items))
        }
        "update" => {
            if args.updates.is_empty() {
                return Err(ToolError::msg(
                    "update needs at least one entry in `updates`",
                ));
            }
            let mut lines = Vec::with_capacity(args.updates.len());
            for update in &args.updates {
                let todo = plan
                    .items
                    .iter_mut()
                    .find(|todo| todo.id == update.id)
                    .ok_or_else(|| unknown(&update.id))?;
                if let Some(raw) = &update.status {
                    todo.status = Status::parse(Some(raw))?;
                }
                if let Some(text) = &update.content {
                    todo.content = content(text)?;
                }
                lines.push(format!("{} {}", todo.id, todo.status.name()));
            }
            validate(&plan.items)?;
            let mut out = lines.join(" · ");
            out.push_str(&format!(" · {}", left(&plan.items)));
            if let Some(next) = plan.items.iter().find(|todo| todo.status.open()) {
                out.push_str(&format!(" · next: {} \"{}\"", next.id, next.content));
            }
            out
        }
        "remove" => {
            if args.ids.is_empty() {
                return Err(ToolError::msg("remove needs at least one id in `ids`"));
            }
            for id in &args.ids {
                if !plan.items.iter().any(|todo| &todo.id == id) {
                    return Err(unknown(id));
                }
            }
            plan.items.retain(|todo| !args.ids.contains(&todo.id));
            format!("{} removed · {}", args.ids.join(", "), left(&plan.items))
        }
        "clear" => {
            plan.items.clear();
            plan.next = 1;
            "No todos.".to_string()
        }
        other => {
            return Err(ToolError::msg(format!(
                "unknown todo action {other:?}; use read, set, add, update, remove or clear"
            )))
        }
    };
    if action != "read" {
        save(toolbox, &plan)?;
    }
    Ok(out)
}

fn content(raw: &str) -> ToolResult<String> {
    let text = raw.trim();
    if text.is_empty() {
        return Err(ToolError::msg("every todo needs a non-empty `content`"));
    }
    Ok(text.to_string())
}

fn unknown(id: &str) -> ToolError {
    ToolError::msg(format!(
        "unknown todo id {id:?}; run todo(action: \"read\") to see the ids"
    ))
}

fn left(items: &[Todo]) -> String {
    format!(
        "{} left",
        items.iter().filter(|todo| todo.status.open()).count()
    )
}

/// validate refuses the two states the checklist cannot show: two items in
/// progress at once, and two items with the same text.
fn validate(items: &[Todo]) -> ToolResult<()> {
    let active: Vec<&str> = items
        .iter()
        .filter(|todo| todo.status == Status::InProgress)
        .map(|todo| todo.id.as_str())
        .collect();
    if active.len() > 1 {
        return Err(ToolError::msg(format!(
            "{} are in_progress; keep at most one and set the other to pending",
            active.join(", ")
        )));
    }
    for (index, todo) in items.iter().enumerate() {
        if items[..index]
            .iter()
            .any(|prev| prev.content == todo.content)
        {
            return Err(ToolError::msg(format!(
                "duplicate todo content {:?}; make each item distinct",
                todo.content
            )));
        }
    }
    Ok(())
}

fn render_full(items: &[Todo]) -> String {
    if items.is_empty() {
        return "No todos.".to_string();
    }
    let count = |status: Status| items.iter().filter(|todo| todo.status == status).count();
    let mut out = format!(
        "{} pending · {} in progress · {} completed · {} cancelled\n",
        count(Status::Pending),
        count(Status::InProgress),
        count(Status::Completed),
        count(Status::Cancelled)
    );
    for todo in items {
        out.push_str(&format!(
            "{} {} {}\n",
            todo.id,
            todo.status.mark(),
            todo.content
        ));
    }
    out
}

/// save rewrites the whole file through a temp name and a rename, so a reader
/// never sees half a list.
fn save(toolbox: &Toolbox, plan: &Plan) -> ToolResult<()> {
    let path = store_path(toolbox);
    if plan.items.is_empty() {
        match fs::remove_file(&path) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        }
    }
    let encoded = serde_json::to_vec_pretty(plan)?;
    let tmp = path.with_extension(format!("json.tmp.{}", std::process::id()));
    fs::write(&tmp, encoded)?;
    fs::rename(&tmp, &path)?;
    Ok(())
}

fn load_plan(toolbox: &Toolbox) -> Plan {
    load_path(&store_path(toolbox))
}

fn load_path(path: &Path) -> Plan {
    let Ok(bytes) = fs::read(path) else {
        return Plan {
            next: 1,
            items: Vec::new(),
        };
    };
    if let Ok(plan) = serde_json::from_slice::<Plan>(&bytes) {
        return fix_ids(plan);
    }
    // Lists written before ids existed were a bare array.
    let items = serde_json::from_slice::<Vec<Todo>>(&bytes).unwrap_or_default();
    fix_ids(Plan { next: 1, items })
}

fn fix_ids(mut plan: Plan) -> Plan {
    for (index, todo) in plan.items.iter_mut().enumerate() {
        if todo.id.is_empty() {
            todo.id = format!("t{}", index + 1);
        }
    }
    let highest = plan
        .items
        .iter()
        .filter_map(|todo| todo.id.strip_prefix('t')?.parse::<u64>().ok())
        .max()
        .unwrap_or(0);
    plan.next = plan.next.max(highest + 1);
    plan
}

/// load reads the stored list back; the runner uses it to decide a reminder.
pub fn load(toolbox: &Toolbox) -> Vec<Todo> {
    load_plan(toolbox).items
}

/// load_from is `load` for a caller without a Toolbox (the daemon's reminder).
pub fn load_from(path: &Path) -> Vec<Todo> {
    load_path(path).items
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

    fn call(toolbox: &Toolbox, args: Value) -> ToolResult<String> {
        run(toolbox, &args)
    }

    #[test]
    fn set_assigns_ids_and_add_keeps_them() {
        let (_dir, toolbox) = toolbox();
        let out = call(
            &toolbox,
            json!({"action": "set", "items": [{"content": "one"}, {"content": "two", "status": "in_progress"}]}),
        )
        .unwrap();
        assert_eq!(
            out,
            "1 pending · 1 in progress · 0 completed · 0 cancelled\nt1 [ ] one\nt2 [~] two\n"
        );
        call(
            &toolbox,
            json!({"action": "add", "items": [{"content": "three"}]}),
        )
        .unwrap();
        let ids: Vec<String> = load(&toolbox).into_iter().map(|todo| todo.id).collect();
        assert_eq!(ids, vec!["t1", "t2", "t3"]);
    }

    #[test]
    fn add_after_places_the_new_item_next() {
        let (_dir, toolbox) = toolbox();
        call(
            &toolbox,
            json!({"action": "set", "items": [{"content": "one"}, {"content": "two"}]}),
        )
        .unwrap();
        call(
            &toolbox,
            json!({"action": "add", "after": "t1", "items": [{"content": "middle"}]}),
        )
        .unwrap();
        let contents: Vec<String> = load(&toolbox)
            .into_iter()
            .map(|todo| todo.content)
            .collect();
        assert_eq!(contents, vec!["one", "middle", "two"]);
    }

    #[test]
    fn update_patches_a_batch_in_one_call() {
        let (_dir, toolbox) = toolbox();
        call(
            &toolbox,
            json!({"action": "set", "items": [{"content": "one", "status": "in_progress"}, {"content": "two"}]}),
        )
        .unwrap();
        let out = call(
            &toolbox,
            json!({"action": "update", "updates": [
                {"id": "t1", "status": "completed"},
                {"id": "t2", "status": "in_progress"}
            ]}),
        )
        .unwrap();
        assert_eq!(
            out,
            "t1 completed · t2 in_progress · 1 left · next: t2 \"two\""
        );
        let stored = load(&toolbox);
        assert_eq!(stored[0].status, Status::Completed);
        assert_eq!(stored[1].status, Status::InProgress);
    }

    #[test]
    fn update_and_add_return_one_line() {
        let (_dir, toolbox) = toolbox();
        call(
            &toolbox,
            json!({"action": "set", "items": [{"content": "one"}]}),
        )
        .unwrap();
        let added = call(
            &toolbox,
            json!({"action": "add", "items": [{"content": "two"}]}),
        )
        .unwrap();
        assert!(!added.contains('\n'));
        let updated = call(
            &toolbox,
            json!({"action": "update", "updates": [
                {"id": "t1", "status": "completed"},
                {"id": "t2", "content": "deux"}
            ]}),
        )
        .unwrap();
        assert!(!updated.contains('\n'));
        assert!(updated.contains("next: t2 \"deux\""));
    }

    #[test]
    fn read_returns_the_full_list_and_set_restarts_ids() {
        let (_dir, toolbox) = toolbox();
        call(
            &toolbox,
            json!({"action": "set", "items": [{"content": "one"}, {"content": "two"}]}),
        )
        .unwrap();
        call(&toolbox, json!({"action": "remove", "ids": ["t1"]})).unwrap();
        let out = call(&toolbox, json!({"action": "read"})).unwrap();
        assert!(out.starts_with("1 pending · 0 in progress · 0 completed · 0 cancelled\n"));
        assert!(out.contains("t2 [ ] two"));
        call(
            &toolbox,
            json!({"action": "set", "items": [{"content": "fresh"}]}),
        )
        .unwrap();
        assert_eq!(load(&toolbox)[0].id, "t1");
    }

    #[test]
    fn refuses_two_in_progress_with_a_fix_hint() {
        let (_dir, toolbox) = toolbox();
        let error = call(
            &toolbox,
            json!({"action": "set", "items": [
                {"content": "one", "status": "in_progress"},
                {"content": "two", "status": "in_progress"}
            ]}),
        )
        .unwrap_err();
        assert!(error.to_string().contains("keep at most one"));
    }

    #[test]
    fn refuses_duplicate_content_and_unknown_ids() {
        let (_dir, toolbox) = toolbox();
        assert!(call(
            &toolbox,
            json!({"action": "set", "items": [{"content": "one"}, {"content": "one"}]}),
        )
        .is_err());
        call(
            &toolbox,
            json!({"action": "set", "items": [{"content": "one"}]}),
        )
        .unwrap();
        let error = call(
            &toolbox,
            json!({"action": "update", "updates": [{"id": "t9", "status": "completed"}]}),
        )
        .unwrap_err();
        assert!(error.to_string().contains("todo(action: \"read\")"));
        assert!(call(&toolbox, json!({"action": "remove", "ids": ["t9"]})).is_err());
    }

    #[test]
    fn cancelled_parses_and_counts_as_resolved() {
        let (_dir, toolbox) = toolbox();
        call(
            &toolbox,
            json!({"action": "set", "items": [{"content": "one", "status": "cancelled"}]}),
        )
        .unwrap();
        let stored = load(&toolbox);
        assert_eq!(stored[0].status, Status::Cancelled);
        let out = call(&toolbox, json!({"action": "read"})).unwrap();
        assert!(out.contains("1 cancelled"));
        assert!(out.contains("t1 [–] one"));
    }

    #[test]
    fn ids_are_not_reused_after_remove() {
        let (_dir, toolbox) = toolbox();
        call(
            &toolbox,
            json!({"action": "set", "items": [{"content": "one"}, {"content": "two"}]}),
        )
        .unwrap();
        call(&toolbox, json!({"action": "remove", "ids": ["t1"]})).unwrap();
        call(
            &toolbox,
            json!({"action": "add", "items": [{"content": "three"}]}),
        )
        .unwrap();
        let ids: Vec<String> = load(&toolbox).into_iter().map(|todo| todo.id).collect();
        assert_eq!(ids, vec!["t2", "t3"]);
    }

    #[test]
    fn clear_and_an_empty_set_remove_the_file() {
        let (_dir, toolbox) = toolbox();
        let path = store_path(&toolbox);
        call(
            &toolbox,
            json!({"action": "set", "items": [{"content": "one"}]}),
        )
        .unwrap();
        assert!(path.exists());
        assert_eq!(
            call(&toolbox, json!({"action": "clear"})).unwrap(),
            "No todos."
        );
        assert!(!path.exists());
        call(
            &toolbox,
            json!({"action": "set", "items": [{"content": "one"}]}),
        )
        .unwrap();
        assert_eq!(
            call(&toolbox, json!({"action": "set", "items": []})).unwrap(),
            "No todos."
        );
        assert!(!path.exists());
    }

    #[test]
    fn refuses_a_blank_entry_and_an_unknown_status() {
        let (_dir, toolbox) = toolbox();
        assert!(call(
            &toolbox,
            json!({"action": "set", "items": [{"content": "  "}]}),
        )
        .is_err());
        assert!(call(
            &toolbox,
            json!({"action": "set", "items": [{"content": "x", "status": "later"}]}),
        )
        .is_err());
        assert!(call(&toolbox, json!({"action": "rewrite"})).is_err());
    }

    #[test]
    fn a_list_written_before_ids_still_loads() {
        let (_dir, toolbox) = toolbox();
        let path = store_path(&toolbox);
        fs::write(
            &path,
            br#"[{"content": "one", "status": "completed"}, {"content": "two", "status": "pending"}]"#,
        )
        .unwrap();
        let stored = load(&toolbox);
        assert_eq!(stored[0].id, "t1");
        assert_eq!(stored[1].id, "t2");
        call(
            &toolbox,
            json!({"action": "add", "items": [{"content": "three"}]}),
        )
        .unwrap();
        assert_eq!(load(&toolbox)[2].id, "t3");
    }
}
