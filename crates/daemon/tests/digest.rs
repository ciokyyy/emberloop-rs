#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use emberloop_core::events::{EventKind, SessionEvent};
use emberloop_daemon::digest::{digest, run, DigestArgs};
use emberloop_daemon::Config;
use serde_json::json;
use store::sessions::ReviewerAssignment;
use store::{ArchivalConfig, ArchivalSweeper, DataRoot, EventLog, SessionMeta, SessionsStore};

const DAY_MS: u64 = 24 * 60 * 60 * 1000;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

fn meta(id: &str, updated: u64, parent: Option<&str>) -> SessionMeta {
    SessionMeta {
        id: id.to_string(),
        project_id: "p1".to_string(),
        title: format!("title {id}"),
        model: "model-x".to_string(),
        created_at: updated - 1000,
        updated_at: updated,
        reviewer: parent.map(|parent_id| ReviewerAssignment {
            parent_id: parent_id.to_string(),
            role: "worker".to_string(),
            ticket_path: String::new(),
            starting_point: String::new(),
            tier: None,
            persona: None,
        }),
        ..SessionMeta::default()
    }
}

async fn append(root: &DataRoot, id: &str, events: Vec<(EventKind, serde_json::Value)>) {
    let log = EventLog::open(root.session_events_dir(id)).await.unwrap();
    for (kind, data) in events {
        log.append(SessionEvent::new(id, 0, kind, data))
            .await
            .unwrap();
    }
}

fn write_skill(dir: &std::path::Path, name: &str, origin: &str) {
    let folder = dir.join(name);
    fs::create_dir_all(&folder).unwrap();
    fs::write(
        folder.join("SKILL.md"),
        format!(
            "---\nname: {name}\ndescription: a skill\nmetadata:\n  origin: {origin}\n---\n# {name}\n"
        ),
    )
    .unwrap();
}

#[tokio::test]
async fn a_digest_reports_facts_skills_and_redacts_secrets() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    fs::create_dir_all(&data).unwrap();
    let project = tmp.path().join("proj");
    fs::create_dir_all(&project).unwrap();
    let global = tmp.path().join("global");
    write_skill(&global, "alpha", "agent");
    write_skill(&project.join(".agents/skills"), "beta", "agent");
    fs::write(
        global.join("skill-modules.json"),
        json!({"media": {"about": "audio", "skills": ["alpha"]}}).to_string(),
    )
    .unwrap();
    fs::write(
        data.join("projects.json"),
        json!([{"id": "p1", "name": "P", "root": project}]).to_string(),
    )
    .unwrap();

    let now = now_ms();
    let root = DataRoot::new(data.clone());
    let sessions = SessionsStore::load(root.clone()).await.unwrap();
    sessions
        .insert(meta("ses_live", now - 2000, None))
        .await
        .unwrap();
    sessions
        .insert(meta("ses_archived", now - 4 * DAY_MS, None))
        .await
        .unwrap();
    sessions
        .insert(meta("ses_child", now - 1000, Some("ses_live")))
        .await
        .unwrap();

    append(
        &root,
        "ses_live",
        vec![
            (
                EventKind::Prompt,
                json!({"text": "make it faster, token abcdefghij1234567890"}),
            ),
            (EventKind::SteerDelivered, json!({"text": "focus on tests"})),
            (
                EventKind::ToolStarted,
                json!({"id": "t1", "name": "bash", "input": {"command": "cargo test"}}),
            ),
            (
                EventKind::ToolFinished,
                json!({"id": "t1", "output": "tests pass\nexit code: 0\n"}),
            ),
            (
                EventKind::ToolStarted,
                json!({"id": "t2", "name": "bash", "input": {"command": "false"}}),
            ),
            (
                EventKind::ToolFinished,
                json!({"id": "t2", "output": "error: boom\nexit code: 1\n"}),
            ),
            (
                EventKind::Usage,
                json!({"inputTokens": 100, "outputTokens": 50, "cachedTokens": 10, "cacheCreationTokens": 5}),
            ),
            (EventKind::Compression, json!({"start": 1, "end": 2})),
            (EventKind::Error, json!({"message": "upstream 500"})),
        ],
    )
    .await;
    append(
        &root,
        "ses_archived",
        vec![(EventKind::Prompt, json!({"text": "old work"}))],
    )
    .await;
    append(
        &root,
        "ses_child",
        vec![(EventKind::Prompt, json!({"text": "subagent work"}))],
    )
    .await;

    let sweeper = ArchivalSweeper::new(root.clone(), sessions.clone(), ArchivalConfig::default());
    let report = sweeper.sweep_at(now).await.unwrap();
    assert_eq!(report.archived, vec!["ses_archived".to_string()]);

    fs::write(
        root.session_dir("ses_live").join("skill-catalog.md"),
        "- alpha: a skill\n- media (1 skills): audio. Use skill_search \"media\" to list.\n",
    )
    .unwrap();
    let now_secs = (now - 3000) / 1000;
    let old_secs = (now - 8 * DAY_MS) / 1000;
    fs::write(
        global.join(".usage.jsonl"),
        format!(
            "{{\"time\":{old_secs},\"name\":\"alpha\",\"op\":\"create\",\"result\":\"ok\"}}\n{{\"time\":{now_secs},\"name\":\"alpha\",\"op\":\"view\",\"result\":\"ok\"}}\n"
        ),
    )
    .unwrap();

    let env_file = tmp.path().join("env");
    fs::write(&env_file, "OTHER_TOKEN=abcdefghij1234567890\n").unwrap();
    std::env::set_var("EMBERLOOP_ENV_FILE", &env_file);
    std::env::set_var("EMBERLOOP_GLOBAL_SKILLS", &global);
    std::env::remove_var("EMBERLOOP_SESSION_DIR");

    let config = Config {
        data_root: data.clone(),
        ..Config::default()
    };
    let since = now - 10 * DAY_MS;
    let report = digest(
        &config,
        &DigestArgs {
            since: Some(since),
            limit: None,
            session: None,
            out: None,
        },
    )
    .unwrap();
    assert_eq!(
        report.line,
        format!(
            "data-root: {} window: since={since} through={} shown 2 of 2",
            data.display(),
            now - 2000
        )
    );
    assert_eq!(report.sessions.len(), 2);
    assert_eq!(report.sessions[0].0, "ses_archived");
    assert!(report.sessions[0].1.contains("old work"));

    let live = &report.sessions[1].1;
    assert!(live.contains("title ses_live"), "{live}");
    assert!(live.contains(&project.display().to_string()), "{live}");
    assert!(live.contains("- prompt: make it faster, token [redacted]"));
    assert!(!live.contains("abcdefghij1234567890"));
    assert!(live.contains("1. `bash` in: {\"command\":\"cargo test\"} exit: 0"));
    assert!(live.contains("2. `bash` in: {\"command\":\"false\"} exit: 1 error: error: boom"));
    assert!(live.contains("## Subagent ses_child (worker)"));
    assert!(live.contains("subagent work"));
    assert!(live.contains("- calls: 1"));
    assert!(live.contains("- input tokens: 100"));
    assert!(live.contains("- events: 1"));
    assert!(live.contains("upstream 500"));
    assert!(live.contains("- steer: focus on tests"));

    let index = &report.index;
    assert!(index.contains("| alpha | agent | false |"), "{index}");
    assert!(index.contains("| 1 | 1 | 1 |"), "{index}");
    assert!(index.contains("Unattributed skill changes"));
    assert!(
        index.contains(&global.join("alpha").join("SKILL.md").display().to_string()),
        "{index}"
    );

    let empty = digest(
        &config,
        &DigestArgs {
            since: Some(now + 10_000),
            limit: None,
            session: None,
            out: None,
        },
    )
    .unwrap();
    assert!(empty.line.contains("shown 0 of 0"), "{}", empty.line);
    assert!(
        empty.line.contains(&format!("through={}", now + 10_000)),
        "{}",
        empty.line
    );

    let one = digest(
        &config,
        &DigestArgs {
            since: None,
            limit: Some(1),
            session: Some("ses_archived".to_string()),
            out: None,
        },
    )
    .unwrap();
    assert!(one.line.contains("shown 1 of 1"), "{}", one.line);

    let out = tmp.path().join("out");
    run(
        &config,
        DigestArgs {
            since: Some(since),
            limit: Some(1),
            session: None,
            out: Some(out.clone()),
        },
    )
    .unwrap();
    assert!(out.join("index.md").is_file());
    assert!(out.join("ses_archived.md").is_file());
    assert_eq!(fs::read_dir(&out).unwrap().count(), 2);

    std::env::remove_var("EMBERLOOP_ENV_FILE");
    std::env::remove_var("EMBERLOOP_GLOBAL_SKILLS");
}
