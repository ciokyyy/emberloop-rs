#![allow(clippy::unwrap_used, clippy::expect_used)]
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::json;
use tempfile::TempDir;
use tools::tag::content_tag;
use tools::{McpServer, ToolConfig, Toolbox};

struct Fixture {
    _dir: TempDir,
    root: PathBuf,
    toolbox: Arc<Toolbox>,
}

fn setup() -> Fixture {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("project");
    fs::create_dir(&root).expect("project root");
    let spill = dir.path().join("spill");
    let config = ToolConfig::new(&root, spill);
    let toolbox = Arc::new(Toolbox::new(config).expect("toolbox"));
    Fixture {
        _dir: dir,
        root,
        toolbox,
    }
}

fn fixture_with_threshold(threshold: usize) -> Fixture {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("project");
    fs::create_dir(&root).expect("project root");
    let spill = dir.path().join("spill");
    let config = ToolConfig {
        overflow_threshold: threshold,
        ..ToolConfig::new(&root, spill)
    };
    let toolbox = Arc::new(Toolbox::new(config).expect("toolbox"));
    Fixture {
        _dir: dir,
        root,
        toolbox,
    }
}

fn write(fixture: &Fixture, name: &str, content: &str) -> PathBuf {
    let path = fixture.root.join(name);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("parent");
    }
    fs::write(&path, content).expect("write fixture");
    path
}

fn tag_of(output: &str) -> String {
    let start = output.find('#').expect("tag marker") + 1;
    output[start..start + 4].to_string()
}
fn read(fixture: &Fixture, path: &Path, offset: usize, limit: usize) -> String {
    let args = json!({"path": path.to_string_lossy(), "offset": offset, "limit": limit});
    futures_lite_block(fixture.toolbox.call("read", &args))
}

/// Tiny synchronous driver so tests can stay `#[test]` and fast.
fn futures_lite_block(
    future: impl std::future::Future<Output = tools::ToolResult<String>>,
) -> String {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(future)
        .expect("tool result")
}

fn call(fixture: &Fixture, name: &str, args: serde_json::Value) -> tools::ToolResult<String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(fixture.toolbox.call(name, &args))
}

#[test]
fn read_numbers_lines_and_returns_tag() {
    let fixture = setup();
    let path = write(&fixture, "a.txt", "alpha\nbeta\ngamma\n");
    let output = read(&fixture, &path, 1, 0);
    assert!(
        output.starts_with(&format!("[{}#", path.display())),
        "{output}"
    );
    assert!(output.contains("(lines 1-3)"), "{output}");
    assert!(output.contains("     1\u{2192}alpha"), "{output}");
    assert!(output.contains("     3\u{2192}gamma"), "{output}");
    assert_eq!(tag_of(&output), content_tag(b"alpha\nbeta\ngamma\n"));
}

#[test]
fn read_offset_limit_and_directory_listing() {
    let fixture = setup();
    let path = write(&fixture, "b.txt", "one\ntwo\nthree\nfour\n");
    let output = read(&fixture, &path, 2, 2);
    assert!(output.contains("(lines 2-3)"), "{output}");
    assert!(!output.contains("one"), "{output}");
    assert!(!output.contains("four"), "{output}");

    let listing = read(&fixture, fixture.root.as_path(), 1, 0);
    assert!(
        listing.contains("[") && listing.contains("entries"),
        "{listing}"
    );
    assert!(listing.contains("b.txt"), "{listing}");
}

#[test]
fn read_overflow_spills_and_ctx_pages() {
    let fixture = fixture_with_threshold(50);
    let content = (1..=100)
        .map(|n| format!("line {n}"))
        .collect::<Vec<_>>()
        .join("\n");
    let path = write(&fixture, "big.txt", &content);
    let output = read(&fixture, &path, 1, 0);
    assert!(output.contains("[overflow]"), "{output}");
    let handle = output
        .split_whitespace()
        .find(|word| word.starts_with("ctx:"))
        .expect("ctx handle")
        .split(':')
        .nth(1)
        .expect("handle id")
        .to_string();
    let paged = read(&fixture, Path::new(&format!("ctx:{handle}:2-3")), 1, 0);
    // Spill line 1 is the read header, so rendered lines 2-3 are content lines 1-2.
    assert!(paged.contains("line 1"), "{paged}");
    assert!(paged.contains("line 2"), "{paged}");
    assert!(!paged.contains("line 3"), "{paged}");
}

#[test]
fn read_ctx_forgives_a_dropped_or_added_c_prefix() {
    let fixture = fixture_with_threshold(50);
    let content = (1..=100)
        .map(|n| format!("line {n}"))
        .collect::<Vec<_>>()
        .join("\n");
    let path = write(&fixture, "big.txt", &content);
    let output = read(&fixture, &path, 1, 0);
    let handle = output
        .split_whitespace()
        .find(|word| word.starts_with("ctx:"))
        .and_then(|word| word.split(':').nth(1))
        .expect("handle id")
        .to_string();
    // The proxy mints `c`-prefixed handles beside the tools' bare ones; models mix them up.
    let added = read(&fixture, Path::new(&format!("ctx:c{handle}:2-3")), 1, 0);
    assert!(added.contains("line 1"), "{added}");
}

#[test]
fn read_skill_uri_catalog_and_item() {
    let fixture = setup();
    write(
        &fixture,
        ".agents/skills/my-skill/SKILL.md",
        "---\nname: my-skill\ndescription: A helpful skill.\nmetadata:\n  origin: agent\n---\n\n## Instructions\nDo test.\n",
    );
    write(
        &fixture,
        ".agents/skills/my-skill/references/notes.txt",
        "Subpath note content.\n",
    );

    // List catalog
    let catalog = call(&fixture, "read", json!({"path": "el://skills"})).expect("catalog");
    assert!(catalog.contains("my-skill"), "{catalog}");
    assert!(catalog.contains("A helpful skill"), "{catalog}");

    // Read skill directly via el://skills/<name>
    let skill_out =
        call(&fixture, "read", json!({"path": "el://skills/my-skill"})).expect("skill read");
    assert!(skill_out.contains("Do test"), "{skill_out}");
    assert!(skill_out.contains("my-skill/SKILL.md#"), "{skill_out}");

    // Read skill via skills:// and skill:
    let alt1 = call(&fixture, "read", json!({"path": "skills://my-skill"})).expect("skills://");
    assert!(alt1.contains("Do test"), "{alt1}");
    let alt2 = call(&fixture, "read", json!({"path": "skill:my-skill"})).expect("skill:");
    assert!(alt2.contains("Do test"), "{alt2}");

    // Read skill subpath
    let sub = call(
        &fixture,
        "read",
        json!({"path": "el://skills/my-skill/references/notes.txt"}),
    )
    .expect("subpath read");
    assert!(sub.contains("Subpath note content"), "{sub}");

    // Reading nonexistent skill returns error
    let err = call(&fixture, "read", json!({"path": "el://skills/nonexistent"})).unwrap_err();
    assert!(err.to_string().contains("not found"), "{err}");
}

#[test]
fn jail_blocks_escapes_and_symlinks() {
    let fixture = setup();
    let outside = fixture._dir.path().join("outside.txt");
    fs::write(&outside, "secret").expect("outside");

    let error = call(&fixture, "read", json!({"path": "../outside.txt"}))
        .expect_err("relative escape refused");
    assert!(
        error.to_string().contains("outside the project root"),
        "{error}"
    );

    let error = call(&fixture, "read", json!({"path": outside.to_string_lossy()}))
        .expect_err("absolute escape refused");
    assert!(
        error.to_string().contains("outside the project root"),
        "{error}"
    );

    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&outside, fixture.root.join("link.txt")).expect("symlink");
        let error =
            call(&fixture, "read", json!({"path": "link.txt"})).expect_err("symlink escape");
        assert!(
            error.to_string().contains("outside the project root"),
            "{error}"
        );
    }
}

#[test]
fn read_refuses_binary_and_oversized_files() {
    let fixture = setup();
    let binary = fixture.root.join("bin.dat");
    fs::write(&binary, [0u8, 1, 2, 3]).expect("binary");
    let error = call(&fixture, "read", json!({"path": binary.to_string_lossy()}))
        .expect_err("binary refused");
    assert!(error.to_string().contains("binary"), "{error}");

    let big = fixture.root.join("huge.txt");
    fs::write(&big, vec![b'a'; 2 * 1024 * 1024 + 1]).expect("big");
    let error = call(&fixture, "read", json!({"path": "huge.txt"})).expect_err("size cap");
    assert!(error.to_string().contains("byte read limit"), "{error}");
    assert!(
        error.to_string().contains("grep") && error.to_string().contains("sed -n"),
        "the refusal names a next step: {error}"
    );
}

#[test]
fn grep_respects_gitignore_and_supports_modes() {
    let fixture = setup();
    write(&fixture, ".gitignore", "ignored/\n");
    write(
        &fixture,
        "src/main.rs",
        "fn main() {\n    let needle = 1;\n}\n",
    );
    write(&fixture, "ignored/secret.txt", "needle in ignored\n");
    write(&fixture, "notes.md", "Needle upper\n");

    let output = call(&fixture, "grep", json!({"pattern": "needle"})).expect("grep");
    assert!(output.contains("src/main.rs:2:"), "{output}");
    assert!(!output.contains("ignored"), "{output}");
    assert!(!output.contains("notes.md"), "{output}");

    let insensitive = call(
        &fixture,
        "grep",
        json!({"pattern": "needle", "case_insensitive": true}),
    )
    .expect("grep case");
    assert!(insensitive.contains("notes.md:1:"), "{insensitive}");

    let files = call(
        &fixture,
        "grep",
        json!({"pattern": "needle", "output_mode": "files"}),
    )
    .expect("files mode");
    assert_eq!(files.trim(), "src/main.rs");

    let counts = call(
        &fixture,
        "grep",
        json!({"pattern": "needle", "output_mode": "count"}),
    )
    .expect("count mode");
    assert_eq!(counts.trim(), "src/main.rs:1");

    let context =
        call(&fixture, "grep", json!({"pattern": "needle", "context": 1})).expect("context");
    assert!(context.contains("src/main.rs:1:fn main"), "{context}");
    assert!(context.contains("src/main.rs:3:}"), "{context}");
}

#[test]
fn grep_refuses_a_parameter_it_does_not_have() {
    let fixture = setup();
    write(&fixture, "dots.txt", "a.c\nabc\n");
    let error = call(&fixture, "grep", json!({"pattern": "a.c", "fixed": true}))
        .expect_err("fixed is not a grep parameter");
    assert!(error.to_string().contains("fixed"), "{error}");
}

#[test]
fn grep_records_seen_lines_for_edit() {
    let fixture = setup();
    let content = "alpha\nneedle here\ngamma\n";
    let path = write(&fixture, "seen.txt", content);
    let tag = content_tag(content.as_bytes());
    call(&fixture, "grep", json!({"pattern": "needle"})).expect("grep");

    let patch = format!("[{}#{tag}]\nPUT 2.=2:\n+replaced\n", path.display());
    let output = call(&fixture, "edit", json!({"input": patch})).expect("edit seen line");
    assert!(output.contains("updated successfully"), "{output}");
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "alpha\nreplaced\ngamma\n"
    );

    let other = write(&fixture, "unseen.txt", "one\ntwo\nthree\n");
    let other_tag = content_tag(b"one\ntwo\nthree\n");
    call(&fixture, "grep", json!({"pattern": "two"})).expect("grep");
    let patch = format!("[{}#{other_tag}]\nPUT 3.=3:\n+THREE\n", other.display());
    let error = call(&fixture, "edit", json!({"input": patch})).expect_err("unseen line");
    assert!(error.to_string().contains("did not show"), "{error}");
}

#[test]
fn edit_replaces_and_returns_new_tag() {
    let fixture = setup();
    let path = write(&fixture, "edit.txt", "one\ntwo\nthree\n");
    let output = read(&fixture, &path, 1, 0);
    let tag = tag_of(&output);
    let patch = format!("[{}#{tag}]\nPUT 2.=2:\n+TWO\n", path.display());
    let reply = call(&fixture, "edit", json!({"input": patch})).expect("edit");
    assert!(
        reply.starts_with(&format!("[{}#", path.display())),
        "{reply}"
    );
    assert!(reply.contains("updated successfully (3 lines)"), "{reply}");
    assert_eq!(fs::read_to_string(&path).unwrap(), "one\nTWO\nthree\n");
    assert_eq!(tag_of(&reply), content_tag(b"one\nTWO\nthree\n"));
    assert!(reply.contains("TWO"), "{reply}");
}

#[test]
fn edit_accepts_bracketed_route_paths() {
    let fixture = setup();
    let path = write(&fixture, "routes/[slug]/page.txt", "one\ntwo\n");
    let tag = tag_of(&read(&fixture, &path, 1, 0));
    let patch = format!("[{}#{tag}]\nPUT 2.=2:\n+TWO\n", path.display());
    call(&fixture, "edit", json!({"input": patch})).expect("bracketed path");
    assert_eq!(fs::read_to_string(&path).unwrap(), "one\nTWO\n");
}

#[test]
fn edit_applies_all_ops_against_original_numbering() {
    let fixture = setup();
    let path = write(&fixture, "ops.txt", "a\nb\nc\n");
    let tag = tag_of(&read(&fixture, &path, 1, 0));
    let patch = format!(
        "[{}#{tag}]\nCUT 1\nPUT <2:\n+X\nPUT >2:\n+Y\nPUT 3.=3:\n+CC\nPUT >$:\n+Z\n",
        path.display()
    );
    let reply = call(&fixture, "edit", json!({"input": patch})).expect("edit all ops");
    assert!(reply.contains("updated successfully"), "{reply}");
    assert_eq!(fs::read_to_string(&path).unwrap(), "X\nb\nY\nCC\nZ\n");
}

#[test]
fn edit_refuses_stale_tags_with_current_lines() {
    let fixture = setup();
    let path = write(&fixture, "stale.txt", "one\ntwo\n");
    let old_tag = tag_of(&read(&fixture, &path, 1, 0));
    fs::write(&path, "one\nchanged\n").expect("external change");
    let patch = format!("[{}#{old_tag}]\nPUT 1.=1:\n+X\n", path.display());
    let error = call(&fixture, "edit", json!({"input": patch})).expect_err("stale");
    let message = error.to_string();
    assert!(message.contains("stale tag"), "{message}");
    assert!(message.contains("changed"), "{message}");
    assert!(
        message.contains(&content_tag(b"one\nchanged\n")),
        "{message}"
    );
}

#[test]
fn edit_rebases_stale_tag_when_change_is_distant() {
    let fixture = setup();
    let path = write(&fixture, "rebase.txt", "one\ntwo\nthree\nfour\nfive\n");
    let old_tag = tag_of(&read(&fixture, &path, 1, 0));
    fs::write(&path, "zero\none\ntwo\nthree\nfour\nfive\n").expect("external insert at top");
    let patch = format!("[{}#{old_tag}]\nPUT 5.=5:\n+FIVE\n", path.display());
    let reply = call(&fixture, "edit", json!({"input": patch})).expect("rebased edit");
    assert!(reply.contains("Warnings: tag"), "{reply}");
    assert!(reply.contains("lines shifted by +1"), "{reply}");
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "zero\none\ntwo\nthree\nfour\nFIVE\n"
    );
}

#[test]
fn edit_refuses_unseen_and_overlapping_ops() {
    let fixture = setup();
    let path = write(&fixture, "lines.txt", "1\n2\n3\n4\n");
    let tag = tag_of(&read(&fixture, &path, 1, 1));
    let unseen = format!("[{}#{tag}]\nPUT 1.=1:\n+X\nPUT 3.=3:\n+Z\n", path.display());
    let error = call(&fixture, "edit", json!({"input": unseen})).expect_err("unseen");
    assert!(error.to_string().contains("did not show"), "{error}");

    let tag = tag_of(&read(&fixture, &path, 1, 0));
    let overlap = format!("[{}#{tag}]\nPUT 1.=3:\n+A\nPUT 2.=4:\n+B\n", path.display());
    let error = call(&fixture, "edit", json!({"input": overlap})).expect_err("overlap");
    assert!(error.to_string().contains("overlap"), "{error}");
}

#[test]
fn edit_rejects_bad_grammar() {
    let fixture = setup();
    let path = write(&fixture, "grammar.txt", "one\n");
    let tag = tag_of(&read(&fixture, &path, 1, 0));

    let missing = format!("[{}#{tag}]\n", path.display());
    let error = call(&fixture, "edit", json!({"input": missing})).expect_err("no ops");
    assert!(error.to_string().contains("no operations"), "{error}");

    let placeholder = format!("[{}#TAG]\nPUT 1.=1:\n+x\n", path.display());
    let error = call(&fixture, "edit", json!({"input": placeholder})).expect_err("placeholder");
    assert!(error.to_string().contains("four hex digits"), "{error}");

    let no_body = format!("[{}#{tag}]\nPUT 1.=1:\n", path.display());
    let error = call(&fixture, "edit", json!({"input": no_body})).expect_err("no body");
    assert!(error.to_string().contains("body line"), "{error}");

    let body_on_op = format!("[{}#{tag}]\nPUT 1.=1: x\n", path.display());
    let error = call(&fixture, "edit", json!({"input": body_on_op})).expect_err("body on op");
    assert!(error.to_string().contains("unexpected line"), "{error}");
}

#[test]
fn bash_merges_output_and_reports_exit_code() {
    let fixture = setup();
    let output = call(
        &fixture,
        "bash",
        json!({"command": "echo hello; echo oops >&2; exit 3"}),
    )
    .expect("bash");
    assert!(output.contains("exit code: 3"), "{output}");
    assert!(output.contains("hello"), "{output}");
    assert!(output.contains("oops"), "{output}");
}

#[test]
fn bash_tells_its_command_which_session_it_runs_in() {
    let fixture = setup();
    let output = call(
        &fixture,
        "bash",
        json!({"command": "printf %s \"$EMBERLOOP_SESSION_DIR\""}),
    )
    .expect("bash");
    let session_dir = fixture.toolbox.spill_dir().parent().unwrap();
    assert!(
        output.contains(&session_dir.display().to_string()),
        "{output}"
    );
}

#[test]
fn bash_timeout_kills_process_group() {
    let fixture = setup();
    let started = std::time::Instant::now();
    let output = call(
        &fixture,
        "bash",
        json!({"command": "sleep 5", "timeout": 1}),
    )
    .expect("bash timeout");
    assert!(output.starts_with("timeout after 1s"), "{output}");
    assert!(started.elapsed() < std::time::Duration::from_secs(4));
}

#[test]
fn bash_cat_feeds_seen_lines() {
    let fixture = setup();
    let content = "one\ntwo\nthree\n";
    let path = write(&fixture, "cat.txt", content);
    call(&fixture, "bash", json!({"command": "cat cat.txt"})).expect("bash cat");
    let tag = content_tag(content.as_bytes());
    let patch = format!("[{}#{tag}]\nPUT 3.=3:\n+THREE\n", path.display());
    let reply = call(&fixture, "edit", json!({"input": patch})).expect("edit after cat");
    assert!(reply.contains("updated successfully"), "{reply}");
}

#[test]
fn bash_output_spills() {
    let fixture = fixture_with_threshold(100);
    let output = call(&fixture, "bash", json!({"command": "seq 1 100"})).expect("bash spill");
    assert!(output.contains("[overflow]"), "{output}");
    assert!(output.contains("ctx:"), "{output}");
}

#[test]
fn background_start_wait_read_list_and_stop() {
    let fixture = setup();
    let started = call(
        &fixture,
        "bash",
        json!({"command": "echo first; sleep 0.3; echo last", "background": true}),
    )
    .expect("background start");
    assert!(started.contains("started background command"), "{started}");
    assert!(started.contains("first"), "{started}");
    let id = started
        .split_whitespace()
        .nth(3)
        .expect("handle")
        .to_string();

    let waited = call(&fixture, "background", json!({"action": "wait", "id": id})).expect("wait");
    assert!(waited.contains("exited (0)"), "{waited}");

    let log = call(&fixture, "background", json!({"action": "read", "id": id})).expect("read");
    assert!(log.contains("last"), "{log}");
    assert!(log.contains("[next offset:"), "{log}");

    let listed = call(&fixture, "background", json!({"action": "list"})).expect("list");
    assert!(listed.contains(&id), "{listed}");
    assert!(listed.contains("exited(0)"), "{listed}");

    let stopped = call(
        &fixture,
        "bash",
        json!({"command": "sleep 10", "background": true}),
    )
    .expect("stop start");
    let stop_id = stopped
        .split_whitespace()
        .nth(3)
        .expect("handle")
        .to_string();
    let stopped = call(
        &fixture,
        "background",
        json!({"action": "stop", "id": stop_id}),
    )
    .expect("stop");
    assert!(stopped.contains("stopped"), "{stopped}");
}

#[test]
fn mcp_serves_tools_and_errors() {
    let fixture = setup();

    write(&fixture, "mcp.txt", "hello\n");
    let server = McpServer::new(fixture.toolbox.clone());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");

    let names: Vec<String> = server
        .tools()
        .into_iter()
        .map(|tool| tool.name.to_string())
        .collect();
    assert_eq!(
        names,
        vec![
            "read",
            "grep",
            "edit",
            "bash",
            "background",
            "write",
            "find",
            "todo",
            "websearch",
            "ctx_search",
            "skill_search",
            "skill_manage",
            "prompt_propose",
            "execute"
        ]
    );

    let call = runtime.block_on(server.call(
        "read",
        Some(serde_json::from_value(json!({"path": "mcp.txt"})).expect("arguments")),
    ));
    let call = serde_json::to_value(call).expect("call result");
    assert_eq!(call["isError"], false);
    assert!(call["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("hello"));

    let bad_tool = runtime.block_on(server.call(
        "nope",
        Some(serde_json::from_value(json!({})).expect("arguments")),
    ));
    let bad_tool = serde_json::to_value(bad_tool).expect("bad tool result");
    assert_eq!(bad_tool["isError"], true);
}

#[test]
fn background_survives_a_daemon_restart_by_pid_adoption() {
    let fixture = setup();
    let started = call(
        &fixture,
        "bash",
        json!({"command": "sleep 30", "background": true}),
    )
    .expect("background start");
    let id = started
        .split_whitespace()
        .nth(3)
        .expect("handle")
        .to_string();

    let config = ToolConfig::new(&fixture.root, fixture.toolbox.spill_dir());
    let restarted = Toolbox::new(config).expect("restarted toolbox");
    let list = futures_lite_block(restarted.call("background", &json!({"action": "list"})));
    assert!(list.contains(&id), "{list}");
    assert!(list.contains("running"), "{list}");

    let read =
        futures_lite_block(restarted.call("background", &json!({"action": "read", "id": id})));
    assert!(read.contains("[next offset:"), "{read}");

    let stopped =
        futures_lite_block(restarted.call("background", &json!({"action": "stop", "id": id})));
    assert!(stopped.contains("stopped"), "{stopped}");
}

#[test]
fn seen_lines_survive_a_restart() {
    let fixture = setup();
    let path = write(&fixture, "seen.txt", "alpha\nbeta\n");
    let shown = read(&fixture, &path, 1, 0);
    let tag = tag_of(&shown);

    let config = ToolConfig::new(&fixture.root, fixture.toolbox.spill_dir());
    let restarted = Toolbox::new(config).expect("restarted toolbox");
    let patch = format!("[{}#{}]\nPUT 2.=2:\n+changed\n", path.display(), tag);
    let output = futures_lite_block(restarted.call("edit", &json!({"input": patch})));
    assert!(output.contains("updated successfully"), "{output}");
    assert_eq!(fs::read_to_string(&path).unwrap(), "alpha\nchanged\n");
}

fn restart_with_meta(fixture: &Fixture, id: &str, field: &str) -> (String, String) {
    let meta_path = fixture.toolbox.spill_dir().join(format!("{id}.json"));
    let mut meta: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&meta_path).unwrap()).unwrap();
    meta[field] = serde_json::Value::String("different".into());
    fs::write(&meta_path, serde_json::to_string(&meta).unwrap()).unwrap();
    let config = ToolConfig::new(&fixture.root, fixture.toolbox.spill_dir());
    let restarted = Toolbox::new(config).expect("restarted toolbox");
    let list = futures_lite_block(restarted.call("background", &json!({"action": "list"})));
    let stop =
        futures_lite_block(restarted.call("background", &json!({"action": "stop", "id": id})));
    (list, stop)
}

fn kill_group(pid: &str) {
    let _ = std::process::Command::new("kill")
        .args(["-9", "--", &format!("-{pid}")])
        .status();
}

#[test]
fn background_rejects_a_mismatched_start_time() {
    let fixture = setup();
    let started = call(
        &fixture,
        "bash",
        json!({"command": "sleep 30", "background": true}),
    )
    .expect("background start");
    let id = started.split_whitespace().nth(3).unwrap().to_string();
    let (list, stop) = restart_with_meta(&fixture, &id, "start_time");
    assert!(list.contains(&id), "{list}");
    assert!(list.contains("lost"), "{list}");
    assert!(stop.contains("not killed"), "{stop}");
    let pid = list
        .split("pid ")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap();
    kill_group(pid);
}

#[test]
fn background_rejects_a_mismatched_boot_id() {
    let fixture = setup();
    let started = call(
        &fixture,
        "bash",
        json!({"command": "sleep 30", "background": true}),
    )
    .expect("background start");
    let id = started.split_whitespace().nth(3).unwrap().to_string();
    let (list, stop) = restart_with_meta(&fixture, &id, "boot_id");
    assert!(list.contains("lost"), "{list}");
    assert!(stop.contains("not killed"), "{stop}");
    let pid = list
        .split("pid ")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap();
    kill_group(pid);
}

#[test]
fn an_exit_while_down_notices_exactly_once() {
    let fixture = setup();
    let started = call(
        &fixture,
        "bash",
        json!({"command": "true", "background": true}),
    )
    .expect("background start");
    let pid = started
        .split("pid ")
        .nth(1)
        .and_then(|rest| rest.split(')').next())
        .expect("pid");
    wait_until_exited(pid);

    let config = ToolConfig::new(&fixture.root, fixture.toolbox.spill_dir());
    let restarted = Toolbox::new(config.clone()).expect("restarted toolbox");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let notices = runtime.block_on(restarted.background().take_notices());
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert!(notices[0].contains("exited (code unknown)"), "{notices:?}");

    let again = Toolbox::new(config).expect("third toolbox");
    let notices = runtime.block_on(again.background().take_notices());
    assert!(notices.is_empty(), "{notices:?}");
}

fn wait_until_exited(pid: &str) {
    let path = format!("/proc/{pid}/stat");
    for _ in 0..100_000 {
        let Ok(stat) = fs::read_to_string(&path) else {
            return;
        };
        let state = stat
            .rfind(')')
            .and_then(|end| stat[end + 1..].split_whitespace().next());
        if state == Some("Z") {
            return;
        }
        std::thread::yield_now();
    }
    panic!("process {pid} did not become a zombie");
}

#[test]
fn ctx_search_finds_spilled_output() {
    let fixture = fixture_with_threshold(10);
    let big = "filler line\n".repeat(30) + "the rare needle sits here\n";
    write(&fixture, "big.txt", &big);
    let spilled = call(&fixture, "bash", json!({"command": "cat big.txt"})).expect("bash");
    assert!(spilled.contains("ctx:"), "{spilled}");
    let found = call(&fixture, "ctx_search", json!({"queries": ["needle"]})).expect("search");
    assert!(found.contains("needle sits here"), "{found}");
}

fn fixture_with_env_file(content: &str) -> Fixture {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("project");
    fs::create_dir(&root).expect("project root");
    let spill = dir.path().join("spill");
    let env_file = dir.path().join("env");
    fs::write(&env_file, content).expect("env file");
    let config = ToolConfig {
        env_file: Some(env_file),
        ..ToolConfig::new(&root, spill)
    };
    let toolbox = Arc::new(Toolbox::new(config).expect("toolbox"));
    Fixture {
        _dir: dir,
        root,
        toolbox,
    }
}

#[test]
fn a_saved_tool_key_reaches_the_tool_without_any_environment() {
    let dir = tempfile::tempdir().unwrap();
    let root_dir = dir.path().join("project");
    fs::create_dir(&root_dir).expect("project root");
    let keys = dir.path().join("tool-keys.json");
    fs::write(&keys, r#"{"EXA_API_KEY":"from-file"}"#).expect("keys file");
    let config = ToolConfig {
        tool_keys_file: Some(keys),
        ..ToolConfig::new(&root_dir, dir.path().join("spill"))
    };
    let toolbox = Toolbox::new(config).expect("toolbox");
    assert_eq!(
        toolbox.tool_key("EXA_API_KEY"),
        Some("from-file".to_string())
    );
    assert_eq!(toolbox.tool_key("BRAVE_API_KEY"), None);
}

#[test]
fn bash_loads_the_runtime_env_file_on_every_command() {
    let fixture = fixture_with_env_file("export EMBERLOOP_TEST_TOKEN=abc\n");
    let output = call(
        &fixture,
        "bash",
        json!({"command": "printf %s \"$EMBERLOOP_TEST_TOKEN\""}),
    )
    .expect("bash env");
    assert!(output.contains("abc"), "{output}");
}

#[test]
fn background_loads_the_runtime_env_file_too() {
    let fixture = fixture_with_env_file("export EMBERLOOP_TEST_TOKEN=abc\n");
    let started = call(
        &fixture,
        "bash",
        json!({"command": "printf %s \"$EMBERLOOP_TEST_TOKEN\"", "background": true}),
    )
    .expect("background env");
    assert!(started.contains("abc"), "{started}");
}

#[test]
fn bash_routes_python_to_execute() {
    let fixture = setup();
    let error = call(
        &fixture,
        "bash",
        json!({"command": "python3 -c 'print(1)'"}),
    )
    .expect_err("python -c");
    assert!(error.to_string().contains("Blocked:"), "{error}");
    assert!(error.to_string().contains("execute tool"), "{error}");
    let heredoc = call(
        &fixture,
        "bash",
        json!({"command": "python3 <<'PY'\nprint(1)\nPY"}),
    )
    .expect_err("python heredoc");
    assert!(heredoc.to_string().contains("execute tool"), "{heredoc}");
}

#[test]
fn bash_leaves_ordinary_python_runs_alone() {
    let fixture = setup();
    let output =
        call(&fixture, "bash", json!({"command": "python3 --version"})).expect("python version");
    assert!(output.contains("Python"), "{output}");
}

#[test]
fn bash_routes_in_place_editors_to_edit() {
    let fixture = setup();
    write(&fixture, "notes.txt", "one\n");
    for command in [
        "sed -i 's/one/two/' notes.txt",
        "perl -pi -e 's/one/two/' notes.txt",
        "awk -i inplace '{print}' notes.txt",
    ] {
        let error = call(&fixture, "bash", json!({"command": command})).expect_err(command);
        assert!(
            error.to_string().contains("edit tool"),
            "{command}: {error}"
        );
    }
    assert_eq!(
        fs::read_to_string(fixture.root.join("notes.txt")).unwrap(),
        "one\n"
    );
}

#[test]
fn bash_routes_file_writes_to_write_and_leaves_quoted_arrows_alone() {
    let fixture = setup();
    let error =
        call(&fixture, "bash", json!({"command": "echo hi > out.txt"})).expect_err("redirect");
    assert!(error.to_string().contains("write tool"), "{error}");
    assert!(!fixture.root.join("out.txt").exists());
    let error = call(
        &fixture,
        "bash",
        json!({"command": "echo hi | tee out.txt"}),
    )
    .expect_err("tee");
    assert!(error.to_string().contains("write tool"), "{error}");
    let output = call(
        &fixture,
        "bash",
        json!({"command": "echo 'a -> b'; ls missing 2>/dev/null; true"}),
    )
    .expect("quoted arrow");
    assert!(output.contains("a -> b"), "{output}");
}

#[test]
fn bash_routes_long_running_commands_to_background() {
    let fixture = setup();
    for command in ["nohup sleep 1", "sleep 5 &"] {
        let error = call(&fixture, "bash", json!({"command": command})).expect_err(command);
        assert!(
            error.to_string().contains("background tool"),
            "{command}: {error}"
        );
    }
    call(&fixture, "bash", json!({"command": "true && true"})).expect("plain chain");
}

#[test]
fn prompt_propose_stages_a_proposal_and_never_changes_the_active_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    fs::create_dir(&root).expect("project root");
    let prompts_dir = dir.path().join("prompts");
    let config = ToolConfig {
        prompts_dir: Some(prompts_dir.clone()),
        ..ToolConfig::new(&root, dir.path().join("spill"))
    };
    let fixture = Fixture {
        toolbox: Arc::new(Toolbox::new(config).expect("toolbox")),
        root,
        _dir: dir,
    };

    let staged = call(
        &fixture,
        "prompt_propose",
        json!({"id": "tool.edit.description", "text": "new edit text", "reason": "bodies ate lines"}),
    )
    .expect("propose");
    assert!(staged.contains("tool.edit.description"), "{staged}");
    assert!(staged.contains("User approves"), "{staged}");
    let active = call(
        &fixture,
        "prompt_propose",
        json!({"id": "tool.edit.description"}),
    )
    .expect("read active");
    assert_ne!(active, "new edit text");
    assert!(active.contains("hashline"), "{active}");
    let file = prompts_dir
        .with_file_name("prompt-proposals")
        .join("tool.edit.description.json");
    assert!(fs::read_to_string(file).unwrap().contains("new edit text"));
    assert!(!prompts_dir.join("tool.edit.description.md").exists());

    for bad in [
        json!({"id": "nope", "text": "t", "reason": "r"}),
        json!({"id": "tool.edit.description", "text": "t"}),
        json!({"id": "tool.edit.description", "text": "t {{ bogus }}", "reason": "r"}),
    ] {
        assert!(call(&fixture, "prompt_propose", bad).is_err());
    }
}

#[test]
fn prompt_propose_without_a_prompts_dir_says_so() {
    let fixture = setup();
    let error = call(
        &fixture,
        "prompt_propose",
        json!({"id": "tool.edit.description", "text": "t", "reason": "r"}),
    )
    .unwrap_err();
    assert!(error.to_string().contains("prompts directory"), "{error}");
}

#[test]
fn max_call_secs_caps_every_blocking_wait_below_the_harness_limit() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    fs::create_dir(&root).expect("project root");
    let config = ToolConfig {
        max_call_secs: Some(1),
        ..ToolConfig::new(&root, dir.path().join("spill"))
    };
    let fixture = Fixture {
        toolbox: Arc::new(Toolbox::new(config).expect("toolbox")),
        root,
        _dir: dir,
    };

    let output = call(
        &fixture,
        "bash",
        json!({"command": "sleep 5", "timeout": 600}),
    )
    .unwrap();
    assert!(output.starts_with("timeout after 1s"), "{output}");

    let started = call(
        &fixture,
        "bash",
        json!({"command": "sleep 5", "background": true}),
    )
    .unwrap();
    let id = started.split_whitespace().nth(3).unwrap().to_string();
    let waited = call(
        &fixture,
        "background",
        json!({"action": "wait", "id": id, "timeout": 600}),
    )
    .unwrap();
    assert!(waited.starts_with("timeout after 1s"), "{waited}");
    call(&fixture, "background", json!({"action": "stop", "id": id})).unwrap();
}
