//! Turn-end todo reminders: the daemon's decision, the steer queue's delivery.
//!
//! Pure functions over the stored list and one per-Session cycle, so the rules
//! are testable without a clock, a harness or a running Session.

use api::state::Settings;
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

/// One line: the count, then each open id with its status and text.
pub fn text(items: &[Todo], sent: u32, max: u32) -> String {
    let open: Vec<String> = items
        .iter()
        .filter(|todo| todo.status.open())
        .map(|todo| format!("{} {} {:?}", todo.id, todo.status.name(), todo.content))
        .collect();
    format!(
        "Reminder {sent} of {max}: {} still open. Mark each item completed or cancelled with \
         todo(action: \"update\") when its work is done, or say what blocks it.",
        open.join(", ")
    )
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
        let line = text(&items, 2, 3);
        assert!(!line.contains('\n'));
        assert!(line.starts_with("Reminder 2 of 3: "));
        assert!(line.contains("t1 pending \"read the spec\""));
        assert!(line.contains("t2 in_progress \"write the parser\""));
        assert!(!line.contains("t3"));
        // The same list and count always render the same bytes: the replay test
        // in ticket 09 depends on it.
        assert_eq!(line, text(&items, 2, 3));
    }

    #[test]
    fn the_cap_note_names_the_open_count() {
        assert_eq!(
            cap_note(2, 3),
            "Todo reminders paused: 3 of 3 sent this turn, 2 still open."
        );
    }
}
