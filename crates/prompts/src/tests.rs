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
        ..Default::default()
    }
}

/// Replaces the override in one rename. A plain write truncates the file first, and the
/// polling watcher can read that empty file: an empty override is the default text, so the
/// watcher would fall back to the default instead of keeping the last good text.
fn write_override(dir: &Path, id: &str, text: &str) {
    std::fs::create_dir_all(dir).unwrap();
    let staged = dir.join(format!(".{id}.md.staged"));
    std::fs::write(&staged, text).unwrap();
    std::fs::rename(&staged, dir.join(format!("{id}.md"))).unwrap();
}

#[test]
fn tool_guidance_keeps_edits_anchored_and_graph_calls_discoverable() {
    assert!(default_text(POLICY).contains("Use hashline `edit` for existing files"));
    assert!(default_text(TOOL_EDIT).contains("PUT 4.=4:"));
    // Python edits route through the hashline tool, not around it.
    assert!(default_text(TOOL_EXECUTE).contains("tools.edit(input="));
    assert!(default_text(SKILL_CODEBASE_MEMORY).contains("tools.call('graph.search_graph'"));
    assert!(default_text(SKILL_CODEBASE_MEMORY).contains("not a complete runtime call graph"));
}

/// Tools the harness is offered, as `tools::mcp` declares them. The prompts crate
/// cannot depend on the tools crate (it is far heavier and `tool_specs` is private),
/// so this list is pinned here and must be kept in step with that declaration.
const OFFERED_TOOLS: &[&str] = &[
    "read",
    "grep",
    "edit",
    "bash",
    "background",
    "write",
    "find",
    "todo",
    "websearch",
    "git",
    "check",
    "ctx_search",
    "execute",
];
/// Tools the proxy injects into every request.
const PROXY_TOOLS: &[&str] = &["compress", "decompress", "search_context", "acp_status"];
/// Bare identifiers the model-facing texts may quote that are neither tools nor
/// tool arguments: the `compress` range fields and the `search_context` arguments.
const ARGUMENTS: &[&str] = &[
    "ranges", "start", "end", "summary", "topic", "contents", "query", "limit",
];

fn backticked(text: &str) -> Vec<&str> {
    text.split('`').skip(1).step_by(2).collect()
}

#[test]
fn every_offered_tool_is_named_in_policy_or_has_a_description_id() {
    let policy = default_text(POLICY);
    for name in OFFERED_TOOLS.iter().chain(PROXY_TOOLS) {
        let id = format!("tool.{name}.description");
        assert!(
            policy.contains(&format!("`{name}`")) || PROMPT_IDS.contains(&id.as_str()),
            "{name} is offered but neither named in policy nor described by {id}"
        );
    }
}

/// Shell commands the prompts quote as examples of output to drop; they are not tools.
const SHELL_COMMANDS: &[&str] = &["npm", "ls"];

#[test]
fn backticked_identifiers_in_model_text_name_a_real_tool_or_argument() {
    let texts = [
        POLICY,
        COMPRESS,
        SEARCH_CONTEXT,
        NUDGE_FIRST,
        NUDGE_PRESSURE,
        NUDGE_EMERGENCY,
        NUDGE_TIER2,
        NUDGE_TIER3,
    ];
    for id in texts {
        for span in backticked(default_text(id)) {
            let bare = !span.is_empty() && span.chars().all(|c| c.is_ascii_lowercase() || c == '_');
            if bare {
                assert!(
                    OFFERED_TOOLS.contains(&span)
                        || PROXY_TOOLS.contains(&span)
                        || ARGUMENTS.contains(&span)
                        || SHELL_COMMANDS.contains(&span),
                    "`{span}` in {id} is not a known tool or argument"
                );
            }
        }
    }
}

#[test]
fn search_before_decompress_is_stated_only_in_policy() {
    assert!(default_text(POLICY).contains("Call `search_context` before `decompress`"));
    assert!(!default_text(SEARCH_CONTEXT).contains("before decompress"));
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
fn caveman_register_keeps_only_the_ultra_row_and_examples() {
    let dir = tempdir().unwrap();
    let store = PromptStore::new(dir.path());
    let text = caveman_text(&store, &vars()).unwrap();

    assert!(text.starts_with("CAVEMAN MODE ACTIVE — level: ultra"));
    assert!(text.contains("| **ultra** |"));
    assert!(!text.contains("| **lite** |"));
    assert!(!text.contains("| **full** |"));
    assert!(!text.contains("| **wenyan-ultra** |"));
    assert!(text.contains("- ultra: \"Inline obj prop, new ref, re-render. `useMemo`.\""));
    assert!(!text.contains("- full: "));
    assert!(!text.contains("- wenyan-ultra: "));
    // The skill's frontmatter is not model-facing.
    assert!(!text.contains("argument-hint"));
    assert!(text.ends_with(
        "CAVEMAN MODE ACTIVE (ultra). Enforce this reply: Drop articles, filler, \
         pleasantries, hedging, and excess conjunctions. Prefer fragments over full \
         natural-prose sentences. State each fact once. No preamble or recap. \
         Technical terms, code, commands, paths, and errors stay exact."
    ));
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

#[test]
fn nudge_ids_are_listed_and_emergency_has_its_own_header_over_the_pressure_body() {
    assert!(PROMPT_IDS.contains(&NUDGE_FIRST));
    assert!(PROMPT_IDS.contains(&NUDGE_PRESSURE));
    assert!(PROMPT_IDS.contains(&NUDGE_EMERGENCY));
    let emergency = default_text(NUDGE_EMERGENCY);
    assert_ne!(emergency, default_text(NUDGE_PRESSURE));
    assert!(emergency.starts_with("Emergency:"));
    assert!(emergency.contains("truncated by the proxy"));
    assert!(emergency.ends_with(default_text(NUDGE_PRESSURE)));
    assert_ne!(default_text(NUDGE_FIRST), default_text(NUDGE_PRESSURE));
    assert!(default_text(NUDGE_FIRST).starts_with("This is an efficiency nudge"));
    assert!(default_text(NUDGE_PRESSURE).starts_with("⚠️ Context limit reached"));
}

#[test]
fn every_nudge_tells_the_model_to_carry_on_with_its_task_after_folding() {
    for id in [
        NUDGE_FIRST,
        NUDGE_PRESSURE,
        NUDGE_EMERGENCY,
        NUDGE_TIER2,
        NUDGE_TIER3,
    ] {
        assert!(
            default_text(id).contains("then carry on with the task you were doing"),
            "{id} lets the model stop after the fold"
        );
    }
}

#[test]
fn nudges_fold_only_genuinely_consumed_ranges_and_define_protected_tools() {
    let all = [
        NUDGE_FIRST,
        NUDGE_PRESSURE,
        NUDGE_EMERGENCY,
        NUDGE_TIER2,
        NUDGE_TIER3,
    ];
    for id in all {
        let text = default_text(id);
        assert!(
            !text.contains("should be compressed"),
            "{id} asks for every range"
        );
        assert!(
            !text.contains("protected content"),
            "{id} uses an undefined term"
        );
        assert!(
            text.contains("genuinely consumed"),
            "{id} lacks the fold rule"
        );
        assert!(
            text.contains("protected tools"),
            "{id} does not name protected tools"
        );
    }
}

#[test]
fn nudge_templates_keep_raw_placeholders_when_pinned_and_render_per_request() {
    let dir = tempdir().unwrap();
    let store = PromptStore::new(dir.path().join("prompts"));
    let pins = PinnedPrompts::pin(&store, &vars()).unwrap();
    let raw = pins.text(NUDGE_PRESSURE).unwrap();
    assert!(raw.contains("{{ context_breakdown }}"));
    assert!(raw.contains("{{ compressible_ranges }}"));

    let nudge_vars = PromptVars {
        context_breakdown: "Context breakdown: 48.0K tool | 4 text".into(),
        since_last_nudge: "+48.0K since last nudge".into(),
        compressible_ranges: "Compressible ranges (1, oldest first):\n  turns 1–77  77 turns"
            .into(),
        ..vars()
    };
    let rendered = render_text(NUDGE_PRESSURE, raw, &nudge_vars).unwrap();
    assert!(rendered.contains("Context breakdown: 48.0K tool | 4 text"));
    assert!(rendered.contains("turns 1–77"));
    assert!(!rendered.contains("{{"));
}

#[test]
fn todo_and_status_templates_pin_raw_and_render_per_request() {
    let dir = tempdir().unwrap();
    let store = PromptStore::new(dir.path().join("prompts"));
    let pins = PinnedPrompts::pin(&store, &vars()).unwrap();

    let reminder = pins.text(TODO_REMINDER).unwrap();
    assert!(reminder.contains("{{ open_todos }}"), "{reminder}");
    let reminder_vars = PromptVars {
        reminder_sent: "2".into(),
        reminder_max: "3".into(),
        open_todos: "t1 pending \"one\"".into(),
        ..vars()
    };
    let rendered = render_text(TODO_REMINDER, reminder, &reminder_vars).unwrap();
    assert!(
        rendered.starts_with("Reminder 2 of 3: t1 pending \"one\" still open."),
        "{rendered}"
    );

    let report = pins.text(ACP_STATUS_REPORT).unwrap();
    assert!(report.contains("{{ status_ranges }}"), "{report}");
    assert!(PROMPT_IDS.contains(&TODO_REMINDER));
    assert!(PROMPT_IDS.contains(&ACP_STATUS_REPORT));
}
