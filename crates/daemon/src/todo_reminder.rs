//! Turn-end todo reminders: the daemon's decision, the steer queue's delivery.
//!
//! Pure functions over the stored list and one per-Session cycle, so the rules
//! are testable without a clock, a harness or a running Session.

use api::state::Settings;
use prompts::{render_text, PromptError, PromptStore, PromptVars, TODO_REMINDER};
use tools::todo::{Status, Todo};

/// What one prompt cycle has already done: how many reminders went out and the
/// open set the last one named, so an unchanged set is never repeated.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Cycle {
    pub sent: u32,
    pub last_open: Vec<(String, Status)>,
    pub capped: bool,
}

/// Open means the item still asks for work: pending or in progress.
pub fn open_set(items: &[Todo]) -> Vec<(String, Status)> {
    items
        .iter()
        .filter(|todo| todo.status.open())
        .map(|todo| (todo.id.clone(), todo.status))
        .collect()
}

/// The four turn-end rules, minus the settled check the caller makes.
pub fn should_send(settings: &Settings, cycle: &Cycle, items: &[Todo]) -> bool {
    if !settings.todo_reminders || settings.todo_reminders_max == 0 {
        return false;
    }
    let open = open_set(items);
    !open.is_empty() && cycle.sent < settings.todo_reminders_max && open != cycle.last_open
}

/// The feed's cap note goes out once per cycle, when the cap is what stopped the reminders.
pub fn should_cap(settings: &Settings, cycle: &Cycle, open: usize) -> bool {
    !cycle.capped
        && settings.todo_reminders
        && settings.todo_reminders_max > 0
        && cycle.sent >= settings.todo_reminders_max
        && open > 0
}

/// The reminder steer, rendered from the `todo.reminder` template. Each open item
/// is one `id status "content"` entry, the content quoted as Rust's debug form.
pub fn text(
    store: &PromptStore,
    items: &[Todo],
    sent: u32,
    max: u32,
) -> Result<String, PromptError> {
    let open: Vec<String> = items
        .iter()
        .filter(|todo| todo.status.open())
        .map(|todo| format!("{} {} {:?}", todo.id, todo.status.name(), todo.content))
        .collect();
    let vars = PromptVars {
        reminder_sent: sent.to_string(),
        reminder_max: max.to_string(),
        open_todos: open.join(", "),
        ..PromptVars::default()
    };
    let template = store
        .text(TODO_REMINDER)
        .ok_or_else(|| PromptError::NotFound {
            id: TODO_REMINDER.to_string(),
        })?;
    render_text(TODO_REMINDER, &template, &vars)
}

/// The dashed note the feed shows once the cap is reached; it is not sent to the model.
pub fn cap_note(open: usize, max: u32) -> String {
    format!("Todo reminders paused: {max} of {max} sent this turn, {open} still open.")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> Settings {
        Settings::default()
    }

    fn store() -> (tempfile::TempDir, PromptStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = PromptStore::new(dir.path().join("prompts"));
        (dir, store)
    }

    fn todo(id: &str, content: &str, status: Status) -> Todo {
        Todo {
            id: id.to_string(),
            content: content.to_string(),
            status,
        }
    }

    #[test]
    fn open_set_ignores_resolved_items() {
        let items = vec![
            todo("t1", "one", Status::Pending),
            todo("t2", "two", Status::Completed),
            todo("t3", "three", Status::Cancelled),
            todo("t4", "four", Status::InProgress),
        ];
        assert_eq!(
            open_set(&items),
            vec![
                ("t1".to_string(), Status::Pending),
                ("t4".to_string(), Status::InProgress)
            ]
        );
    }

    #[test]
    fn nothing_when_off_empty_or_resolved() {
        let items = vec![todo("t1", "one", Status::Pending)];
        let mut off = settings();
        off.todo_reminders = false;
        assert!(!should_send(&off, &Cycle::default(), &items));
        let mut zero = settings();
        zero.todo_reminders_max = 0;
        assert!(!should_send(&zero, &Cycle::default(), &items));
        assert!(!should_send(&settings(), &Cycle::default(), &[]));
        let done = vec![todo("t1", "one", Status::Completed)];
        assert!(!should_send(&settings(), &Cycle::default(), &done));
    }

    #[test]
    fn an_unchanged_open_set_is_not_repeated() {
        let items = vec![todo("t1", "one", Status::Pending)];
        let mut cycle = Cycle::default();
        assert!(should_send(&settings(), &cycle, &items));
        cycle.sent = 1;
        cycle.last_open = open_set(&items);
        assert!(!should_send(&settings(), &cycle, &items));
        let moved = vec![todo("t1", "one", Status::InProgress)];
        assert!(should_send(&settings(), &cycle, &moved));
    }

    #[test]
    fn the_cap_stops_the_count() {
        let items = vec![todo("t1", "one", Status::Pending)];
        let mut cycle = Cycle {
            sent: 3,
            last_open: Vec::new(),
            capped: false,
        };
        assert!(!should_send(&settings(), &cycle, &items));
        cycle.sent = 2;
        assert!(should_send(&settings(), &cycle, &items));
    }

    #[test]
    fn text_is_one_line_with_the_open_ids() {
        let items = vec![
            todo("t1", "read the spec", Status::Pending),
            todo("t2", "write the parser", Status::InProgress),
            todo("t3", "done", Status::Completed),
        ];
        let (_dir, store) = store();
        let line = text(&store, &items, 2, 3).unwrap();
        assert!(!line.contains('\n'));
        assert!(line.starts_with("Reminder 2 of 3: "));
        assert!(line.contains("t1 pending \"read the spec\""));
        assert!(line.contains("t2 in_progress \"write the parser\""));
        assert!(!line.contains("t3"));
        // The same list and count always render the same bytes: the replay test
        // in ticket 09 depends on it.
        assert_eq!(line, text(&store, &items, 2, 3).unwrap());
    }

    #[test]
    fn reminder_bytes_are_pinned() {
        let (_dir, store) = store();
        let items = vec![
            todo("t1", "read the spec", Status::Pending),
            todo("t2", "write \"the\" parser", Status::InProgress),
            todo("t3", "done", Status::Completed),
        ];
        assert_eq!(
            text(&store, &items, 2, 3).unwrap(),
            "Reminder 2 of 3: t1 pending \"read the spec\", t2 in_progress \"write \\\"the\\\" parser\" still open. Mark each item completed or cancelled with todo(action: \"update\") when its work is done, or say what blocks it."
        );
    }

    #[test]
    fn the_cap_note_is_due_once_when_the_cap_stops_open_items() {
        let mut cycle = Cycle {
            sent: 3,
            last_open: Vec::new(),
            capped: false,
        };
        assert!(should_cap(&settings(), &cycle, 2));
        assert!(!should_cap(&settings(), &cycle, 0));
        cycle.sent = 2;
        assert!(!should_cap(&settings(), &cycle, 2));
        cycle.sent = 3;
        cycle.capped = true;
        assert!(!should_cap(&settings(), &cycle, 2));
    }

    #[test]
    fn the_cap_note_names_the_open_count() {
        assert_eq!(
            cap_note(2, 3),
            "Todo reminders paused: 3 of 3 sent this turn, 2 still open."
        );
    }
}
