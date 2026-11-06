use super::*;
use std::sync::Mutex;
use tempfile::tempdir;

fn vars() -> PromptVars {
    PromptVars {
        project_name: "emberloop".into(),
        project_root: "/tmp/project".into(),
        level: "full".into(),
        date: "2026-11-05".into(),
        session_id: "ses_1".into(),
    }
}

fn write_override(dir: &Path, id: &str, text: &str) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join(format!("{id}.md")), text).unwrap();
}

#[test]
fn default_used_when_no_override_exists() {
    let dir = tempdir().unwrap();
    let store = PromptStore::new(dir.path());
    assert_eq!(store.text(POLICY).as_deref(), Some(default_text(POLICY)));
    assert!(store.errors().is_empty());
}

#[test]
fn override_wins() {
    let dir = tempdir().unwrap();
    write_override(dir.path(), POLICY, "custom policy");
    let store = PromptStore::new(dir.path());
    assert_eq!(store.text(POLICY).as_deref(), Some("custom policy"));
    assert!(store.has_override(POLICY));
}

#[test]
fn empty_override_falls_back_to_default() {
    let dir = tempdir().unwrap();
    write_override(dir.path(), POLICY, "  \n");
    let store = PromptStore::new(dir.path());
    assert_eq!(store.text(POLICY).as_deref(), Some(default_text(POLICY)));
    assert!(!store.has_override(POLICY));
}

#[test]
fn unknown_placeholder_fails_at_load() {
    let dir = tempdir().unwrap();
    write_override(dir.path(), POLICY, "hello {{ nobody }}");
    let store = PromptStore::new(dir.path());
    assert_eq!(store.text(POLICY).as_deref(), Some(default_text(POLICY)));
    assert!(matches!(
        store.errors().first(),
        Some(PromptError::UnknownPlaceholder { .. })
    ));
}

#[test]
fn unterminated_placeholder_fails_at_load() {
    let dir = tempdir().unwrap();
    write_override(dir.path(), POLICY, "hello {{ project_name");
    let store = PromptStore::new(dir.path());
    assert!(matches!(
        store.errors().first(),
        Some(PromptError::UnterminatedPlaceholder { .. })
    ));
}

#[test]
fn broken_override_keeps_last_good_text() {
    let dir = tempdir().unwrap();
    let store = PromptStore::new(dir.path());
    store
        .set_override(POLICY, "good text {{ session_id }}")
        .unwrap();

    write_override(dir.path(), POLICY, "broken {{ nope }}");
    let errors = store.reload();

    assert_eq!(errors.len(), 1);
    assert_eq!(
        store.text(POLICY).as_deref(),
        Some("good text {{ session_id }}")
    );
}

#[test]
fn deleting_override_restores_default() {
    let dir = tempdir().unwrap();
    let store = PromptStore::new(dir.path());
    store.set_override(POLICY, "custom").unwrap();
    assert_eq!(store.text(POLICY).as_deref(), Some("custom"));

    store.remove_override(POLICY).unwrap();
    assert_eq!(store.text(POLICY).as_deref(), Some(default_text(POLICY)));
    assert!(!dir.path().join("policy.md").exists());
}

#[test]
fn placeholders_render_from_vars() {
    let dir = tempdir().unwrap();
    let store = PromptStore::new(dir.path());
    store
        .set_override(
            POLICY,
            "{{ project_name }} at {{ project_root }} ({{ level }})",
        )
        .unwrap();
    let rendered = store.render(POLICY, &vars()).unwrap();
    assert_eq!(rendered, "emberloop at /tmp/project (full)");
}

#[test]
fn pinning_freezes_text_and_hashes() {
    let dir = tempdir().unwrap();
    let store = PromptStore::new(dir.path());
    let running = PinnedPrompts::pin(&store, &vars()).unwrap();
    let before = running.text(POLICY).unwrap().to_string();

    store.set_override(POLICY, "changed after start").unwrap();
    assert_eq!(running.text(POLICY), Some(before.as_str()));

    let new_session = PinnedPrompts::pin(&store, &vars()).unwrap();
    assert_eq!(new_session.text(POLICY), Some("changed after start"));
}

#[test]
fn pinning_records_one_hash_per_prompt() {
    let dir = tempdir().unwrap();
    let store = PromptStore::new(dir.path());
    let pins = PinnedPrompts::pin(&store, &vars()).unwrap();
    assert_eq!(pins.hashes().len(), PROMPT_IDS.len());
    for id in PROMPT_IDS {
        assert_eq!(
            pins.hash(id),
            Some(hash_text(pins.text(id).unwrap()).as_str()),
            "hash of {id}"
        );
    }
}

#[test]
fn ponytail_off_is_empty_and_levels_keep_their_row() {
    let dir = tempdir().unwrap();
    let store = PromptStore::new(dir.path());
    let mut vars = vars();

    vars.level = "off".into();
    assert!(ponytail_text(&store, "off", &vars).unwrap().is_empty());

    let full = ponytail_text(&store, "full", &vars).unwrap();
    assert!(full.contains("| **full** |"));
    assert!(!full.contains("| **lite** |"));
    assert!(!full.contains("| **ultra** |"));
    assert!(full.contains("# Ponytail"));
    assert!(!full.contains("argument-hint"));

    let lite = ponytail_text(&store, "lite", &vars).unwrap();
    assert!(lite.contains("| **lite** |"));
    assert!(!lite.contains("| **full** |"));

    let unknown = ponytail_text(&store, "wat", &vars).unwrap();
    assert!(unknown.contains("| **full** |"));
}

#[test]
fn watcher_sees_change_for_a_new_pin_and_reports_a_broken_file() {
    let dir = tempdir().unwrap();
    let store = Arc::new(PromptStore::new(dir.path()));
    let errors = Arc::new(Mutex::new(Vec::new()));
    let captured = errors.clone();
    let watcher = watch(store.clone(), Duration::from_millis(10), move |error| {
        captured.lock().unwrap().push(error);
    });

    write_override(dir.path(), POLICY, "picked up by watcher");
    wait_until(|| store.text(POLICY).as_deref() == Some("picked up by watcher"));
    let pins = PinnedPrompts::pin(&store, &vars()).unwrap();
    assert_eq!(pins.text(POLICY), Some("picked up by watcher"));

    write_override(dir.path(), POLICY, "{{ broken }}");
    wait_until(|| !errors.lock().unwrap().is_empty());
    assert_eq!(store.text(POLICY).as_deref(), Some("picked up by watcher"));
    watcher.stop();
}

#[test]
fn listing_reports_hashes_and_difference() {
    let dir = tempdir().unwrap();
    let store = PromptStore::new(dir.path());
    store.set_override(POLICY, "custom").unwrap();
    let listing = store.list();
    let policy = listing.iter().find(|info| info.id == POLICY).unwrap();
    assert!(policy.differs);
    assert_eq!(policy.override_hash, Some(hash_text("custom")));
    assert_eq!(policy.default_hash, hash_text(default_text(POLICY)));
    assert!(policy.error.is_none());
    assert_eq!(listing.len(), PROMPT_IDS.len());
}

fn wait_until(mut done: impl FnMut() -> bool) {
    // The watcher is a real thread on a real interval, so the wait is a real
    // deadline: yielding keeps it out of the check's sleep budget.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !done() {
        assert!(
            std::time::Instant::now() < deadline,
            "condition not reached"
        );
        std::thread::yield_now();
    }
}
