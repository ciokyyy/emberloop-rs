//! LLM-free Session digests: facts only, for the retro skill (D32).
//!
//! `emberloop-daemon digest` reads the data root, folds subagent Sessions into their parent and
//! writes Markdown that names what happened: prompts, steers, tool calls, errors and usage, plus
//! skill stats and skill changes that no `write`/`edit` line explains.
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result};
use emberloop_core::events::{EventKind, SessionEvent};
use serde_json::Value;
use store::{DataRoot, Project, SessionMeta};

use crate::Config;

/// Prompt and steer text a digest shows.
const MAX_PROMPT_CHARS: usize = 400;
/// Tool input a digest shows.
const MAX_INPUT_CHARS: usize = 200;
/// Error text a digest shows.
const MAX_ERROR_CHARS: usize = 200;
/// Sessions one digest shows when `--limit` is not given.
const DEFAULT_LIMIT: usize = 20;

pub struct DigestArgs {
    /// Unix milliseconds; a Session counts when its `updatedAt` is greater.
    pub since: Option<u64>,
    pub limit: Option<usize>,
    /// Digest this one Session and ignore `--since` and `--limit`.
    pub session: Option<String>,
    /// Write `index.md` and one file per Session here instead of stdout.
    pub out: Option<PathBuf>,
}

/// The digest of one run: the stdout line, `index.md` and one body per Session.
pub struct Digest {
    pub line: String,
    pub index: String,
    pub sessions: Vec<(String, String)>,
}

/// The CLI: resolves the roots from the environment, then writes the digest.
pub fn run(config: &Config, args: DigestArgs) -> Result<()> {
    write(&Roots::resolve(config), args)
}

/// Writes the digest and prints one window line.
pub fn write(roots: &Roots, args: DigestArgs) -> Result<()> {
    let digest = digest(roots, &args)?;
    if let Some(dir) = &args.out {
        fs::create_dir_all(dir).with_context(|| format!("could not create {}", dir.display()))?;
        fs::write(dir.join("index.md"), &digest.index)
            .with_context(|| format!("could not write {}", dir.join("index.md").display()))?;
        for (id, body) in &digest.sessions {
            fs::write(dir.join(format!("{id}.md")), body)
                .with_context(|| format!("could not write {id}.md"))?;
        }
    }
    println!("{}", digest.line);
    if args.out.is_none() {
        print!("{}", digest.index);
        for (_, body) in &digest.sessions {
            print!("{body}");
        }
    }
    Ok(())
}

/// The roots a digest reads, resolved once from the environment the CLI hands it.
pub struct Roots {
    pub data_root: PathBuf,
    pub global: PathBuf,
    pub env_file: Option<PathBuf>,
}

impl Roots {
    /// `EMBERLOOP_SESSION_DIR` and `EMBERLOOP_GLOBAL_SKILLS` win over the config,
    /// because `bash` hands a Session paths, not a config file.
    pub fn resolve(config: &Config) -> Self {
        let data_root = std::env::var_os("EMBERLOOP_SESSION_DIR")
            .map(PathBuf::from)
            .and_then(|dir| dir.parent().and_then(Path::parent).map(Path::to_path_buf))
            .unwrap_or_else(|| config.data_root.clone());
        let global = std::env::var_os("EMBERLOOP_GLOBAL_SKILLS")
            .map(PathBuf::from)
            .unwrap_or_else(|| config.global_skills.clone());
        Self {
            data_root,
            global,
            env_file: std::env::var_os("EMBERLOOP_ENV_FILE").map(PathBuf::from),
        }
    }
}

/// Builds the digest without writing anything, so a test can inspect it.
pub fn digest(roots: &Roots, args: &DigestArgs) -> Result<Digest> {
    let since = args.since.unwrap_or(0);
    let limit = args.limit.unwrap_or(DEFAULT_LIMIT);
    let root = DataRoot::new(roots.data_root.clone());
    let projects = load_projects(&root);
    let metas = load_metas(&root);
    let (sessions, total) = select(&root, &metas, args, since, limit);
    let through = sessions.last().map_or(since, |(meta, _)| meta.updated_at);
    let window = (since, through);
    let redactor = Redactor::new(roots.env_file.as_deref());
    let mut rendered = Vec::new();
    let mut dirs = Vec::new();
    for (meta, lines) in &sessions {
        let children = children_of(&metas, &meta.id);
        dirs.push(root.session_dir(&meta.id));
        dirs.extend(children.iter().map(|child| root.session_dir(&child.id)));
        rendered.push((
            meta.id.clone(),
            redactor.apply(&render_session(
                &root, meta, lines, &projects, &children, &metas,
            )),
        ));
    }
    let skills = skill_rows(&roots.global, &projects, &dirs, window);
    let index = redactor.apply(&render_index(&skills, window, (sessions.len(), total)));
    let shown = sessions.len();
    let out = args
        .out
        .as_ref()
        .map_or_else(String::new, |dir| format!(" out: {}", dir.display()));
    let line = format!(
        "data-root: {} window: since={since} through={through} shown {shown} of {total}{out}",
        roots.data_root.display()
    );
    Ok(Digest {
        line,
        index,
        sessions: rendered,
    })
}

/// The selected Sessions with their event lines, oldest first, and the match count.
fn select(
    root: &DataRoot,
    metas: &[SessionMeta],
    args: &DigestArgs,
    since: u64,
    limit: usize,
) -> (Vec<(SessionMeta, Vec<String>)>, usize) {
    let explicit = args.session.is_some();
    let mut found: Vec<(SessionMeta, Vec<String>)> = metas
        .iter()
        .filter(|meta| args.session.as_ref().is_none_or(|id| &meta.id == id))
        .filter(|meta| explicit || meta.reviewer.is_none())
        .filter(|meta| explicit || meta.updated_at > since)
        .filter_map(|meta| {
            let lines = store::session_event_lines(root, &meta.id);
            (explicit || !lines.is_empty()).then(|| (meta.clone(), lines))
        })
        .collect();
    found.sort_by_key(|(meta, _)| (meta.updated_at, meta.id.clone()));
    let total = found.len();
    if !explicit {
        found.truncate(limit);
    }
    (found, total)
}

fn children_of<'a>(metas: &'a [SessionMeta], parent: &str) -> Vec<&'a SessionMeta> {
    let mut children: Vec<&SessionMeta> = metas
        .iter()
        .filter(|meta| {
            meta.reviewer
                .as_ref()
                .is_some_and(|reviewer| reviewer.parent_id == parent)
        })
        .collect();
    children.sort_by_key(|meta| (meta.updated_at, meta.id.clone()));
    children
}

fn load_metas(root: &DataRoot) -> Vec<SessionMeta> {
    let Ok(entries) = fs::read_dir(root.sessions_dir()) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| fs::read_to_string(entry.path().join("meta.json")).ok())
        .filter_map(|text| serde_json::from_str::<SessionMeta>(&text).ok())
        .collect()
}

fn load_projects(root: &DataRoot) -> HashMap<String, PathBuf> {
    let Ok(text) = fs::read_to_string(root.projects_file()) else {
        return HashMap::new();
    };
    serde_json::from_str::<Vec<Project>>(&text)
        .unwrap_or_default()
        .into_iter()
        .map(|project| (project.id, project.root))
        .collect()
}

fn render_session(
    root: &DataRoot,
    meta: &SessionMeta,
    lines: &[String],
    projects: &HashMap<String, PathBuf>,
    children: &[&SessionMeta],
    metas: &[SessionMeta],
) -> String {
    let mut out = String::new();
    out.push_str(&format!("# Session {}\n\n", meta.id));
    let project = projects.get(&meta.project_id).map_or_else(
        || format!("{} (missing)", meta.project_id),
        |root| root.display().to_string(),
    );
    out.push_str(&format!("- project: {project}\n"));
    out.push_str(&format!("- title: {}\n", title(meta)));
    out.push_str(&format!("- model: {}\n", meta.model));
    out.push_str(&format!("- created: {}\n", meta.created_at));
    out.push_str(&format!("- updated: {}\n\n", meta.updated_at));
    out.push_str(&render_facts(&facts(lines)));
    for child in children {
        let role = child
            .reviewer
            .as_ref()
            .map_or("subagent", |reviewer| reviewer.role.as_str());
        out.push_str(&format!(
            "\n## Subagent {} ({role})\n\n- updated: {}\n\n",
            child.id, child.updated_at
        ));
        out.push_str(&render_facts(&facts(&store::session_event_lines(
            root, &child.id,
        ))));
        out.push_str(&format!(
            "- children: {}\n",
            children_of(metas, &child.id).len()
        ));
    }
    out
}

fn title(meta: &SessionMeta) -> &str {
    if meta.title.is_empty() {
        "(untitled)"
    } else {
        &meta.title
    }
}

fn render_facts(facts: &Facts) -> String {
    let mut out = String::from("## Prompts and steers\n\n");
    if let Some(prompt) = &facts.first_prompt {
        out.push_str(&format!("- prompt: {prompt}\n"));
    }
    for steer in &facts.steers {
        out.push_str(&format!("- steer: {steer}\n"));
    }
    if facts.first_prompt.is_none() && facts.steers.is_empty() {
        out.push_str("- none\n");
    }
    out.push_str("\n## Tool calls\n\n");
    if facts.tools.is_empty() {
        out.push_str("- none\n");
    }
    for call in &facts.tools {
        let exit = call
            .exit
            .map_or_else(String::new, |code| format!(" exit: {code}"));
        let error = call
            .error
            .as_ref()
            .map_or_else(String::new, |line| format!(" error: {line}"));
        out.push_str(&format!(
            "{}. `{}` in: {}{exit}{error}\n",
            call.seq, call.name, call.input
        ));
    }
    out.push_str("\n## Errors\n\n");
    if facts.errors.is_empty() {
        out.push_str("- none\n");
    }
    for error in &facts.errors {
        out.push_str(&format!("- {error}\n"));
    }
    let usage = &facts.usage;
    out.push_str(&format!(
        "\n## Usage\n\n- calls: {}\n- input tokens: {}\n- output tokens: {}\n- cached tokens: {}\n- cache creation tokens: {}\n\n## Compression\n\n- events: {}\n",
        usage.calls,
        usage.input,
        usage.output,
        usage.cached,
        usage.cache_creation,
        facts.compressions
    ));
    out
}

#[derive(Default)]
struct Facts {
    first_prompt: Option<String>,
    steers: Vec<String>,
    tools: Vec<ToolCall>,
    errors: Vec<String>,
    usage: Usage,
    compressions: usize,
}

#[derive(Default)]
struct Usage {
    calls: u64,
    input: u64,
    output: u64,
    cached: u64,
    cache_creation: u64,
}

struct ToolCall {
    seq: u64,
    name: String,
    input: String,
    exit: Option<i32>,
    error: Option<String>,
}

fn facts(lines: &[String]) -> Facts {
    let mut facts = Facts::default();
    let mut pending: HashMap<String, usize> = HashMap::new();
    for line in lines {
        let Ok(event) = serde_json::from_str::<SessionEvent>(line) else {
            continue;
        };
        match event.kind {
            EventKind::Prompt => {
                if facts.first_prompt.is_none() {
                    facts.first_prompt =
                        text(&event.data, "text").map(|text| cap(&text, MAX_PROMPT_CHARS));
                }
            }
            EventKind::SteerDelivered => {
                if let Some(text) = text(&event.data, "text") {
                    facts.steers.push(cap(&text, MAX_PROMPT_CHARS));
                }
            }
            EventKind::ToolStarted => {
                let id = text(&event.data, "id").unwrap_or_default();
                let name = text(&event.data, "name").unwrap_or_else(|| "tool".to_string());
                let input = event
                    .data
                    .get("input")
                    .map(|value| cap(&value.to_string(), MAX_INPUT_CHARS))
                    .unwrap_or_default();
                pending.insert(id, facts.tools.len());
                facts.tools.push(ToolCall {
                    seq: event.seq,
                    name,
                    input,
                    exit: None,
                    error: None,
                });
            }
            EventKind::ToolFinished => {
                let id = text(&event.data, "id").unwrap_or_default();
                let output = text(&event.data, "output").unwrap_or_default();
                let Some(at) = pending.remove(&id) else {
                    continue;
                };
                let call = &mut facts.tools[at];
                call.exit = exit_status(&output);
                call.error = first_error_line(&output);
            }
            EventKind::Usage => {
                facts.usage.calls += 1;
                facts.usage.input += number(&event.data, "inputTokens");
                facts.usage.output += number(&event.data, "outputTokens");
                facts.usage.cached += number(&event.data, "cachedTokens");
                facts.usage.cache_creation += number(&event.data, "cacheCreationTokens");
            }
            EventKind::Compression => facts.compressions += 1,
            EventKind::Error => {
                if let Some(message) = text(&event.data, "message") {
                    facts.errors.push(cap(&message, MAX_ERROR_CHARS));
                }
            }
            _ => {}
        }
    }
    facts
}

fn text(data: &Value, key: &str) -> Option<String> {
    data.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

fn number(data: &Value, key: &str) -> u64 {
    data.get(key).and_then(Value::as_u64).unwrap_or(0)
}

fn cap(text: &str, limit: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= limit {
        return flat;
    }
    let mut cut: String = flat.chars().take(limit.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

/// The exit status an output text shows, as `exit code: N` or `[exit status N; …]`.
fn exit_status(output: &str) -> Option<i32> {
    for marker in ["exit code:", "[exit status "] {
        let Some(at) = output.find(marker) else {
            continue;
        };
        let digits: String = output[at + marker.len()..]
            .trim_start()
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '-')
            .collect();
        if let Ok(code) = digits.parse() {
            return Some(code);
        }
    }
    None
}

fn first_error_line(output: &str) -> Option<String> {
    let needle = b"error";
    output
        .lines()
        .map(str::trim)
        .find(|line| {
            !line.is_empty()
                && line
                    .as_bytes()
                    .windows(needle.len())
                    .any(|window| window.eq_ignore_ascii_case(needle))
        })
        .map(|line| cap(line, MAX_ERROR_CHARS))
}

/// The window line and the skill sections of `index.md`.
fn render_index(rows: &[SkillRow], window: (u64, u64), shown: (usize, usize)) -> String {
    let (since, through) = window;
    let mut out = format!(
        "# Digest\n\nwindow: since={since} through={through} shown {} of {}\n\n## Skill stats\n\n",
        shown.0, shown.1
    );
    if rows.is_empty() {
        out.push_str("- no skill roots in the window\n");
    } else {
        out.push_str("| root | skill | origin | hidden | bytes | exposures | module-listed | reads | unattributed |\n|---|---|---|---|---|---|---|---|---|\n");
        for row in rows {
            out.push_str(&format!(
                "| {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
                row.root,
                row.name,
                row.origin,
                row.hidden,
                row.bytes,
                row.exposures,
                row.module_listed,
                row.reads,
                row.changes.len()
            ));
        }
    }
    out.push_str("\n## Unattributed skill changes\n\n");
    let changes: Vec<&String> = rows.iter().flat_map(|row| row.changes.iter()).collect();
    if changes.is_empty() {
        out.push_str("- none\n");
    }
    for change in changes {
        out.push_str(&format!("- {change}\n"));
    }
    out
}

struct SkillRow {
    root: String,
    name: String,
    origin: String,
    hidden: bool,
    bytes: u64,
    exposures: usize,
    module_listed: usize,
    reads: u64,
    changes: Vec<String>,
}

/// One row per (skills root, skill) in the window, with the Sessions that met it.
fn skill_rows(
    global: &Path,
    projects: &HashMap<String, PathBuf>,
    session_dirs: &[PathBuf],
    window: (u64, u64),
) -> Vec<SkillRow> {
    let since = window.0;
    let mut roots: Vec<(PathBuf, bool)> = Vec::new();
    if global.is_dir() {
        roots.push((global.to_path_buf(), false));
    }
    let mut project_roots: BTreeSet<PathBuf> = BTreeSet::new();
    for dir in session_dirs {
        if let Some(root) = project_of(projects, dir) {
            project_roots.insert(root);
        }
    }
    for project in &project_roots {
        let root = project.join(tools::skills::PROJECT_SKILLS);
        if root.is_dir() {
            roots.push((root, true));
        }
    }
    let mut module_maps: HashMap<PathBuf, HashMap<String, Vec<String>>> = HashMap::new();
    for project in &project_roots {
        let by_module = tools::skills::modules(Some(global), project)
            .into_iter()
            .map(|module| (module.name, module.skills))
            .collect();
        module_maps.insert(project.clone(), by_module);
    }
    let meetings: Vec<(PathBuf, HashSet<String>, HashSet<String>)> = session_dirs
        .iter()
        .filter_map(|dir| {
            let project = project_of(projects, dir)?;
            let catalog = fs::read_to_string(dir.join("skill-catalog.md")).ok()?;
            let (listed, modules) = listed(&catalog);
            let module_listed = module_members(&modules, &project, &module_maps);
            Some((project, listed, module_listed))
        })
        .collect();
    let mut rows = Vec::new();
    for (root, is_project) in roots {
        let skills = tools::skills::load(&root, is_project);
        let usage = UsageLog::read(&root);
        for skill in skills {
            let bytes = fs::metadata(&skill.path).map_or(0, |meta| meta.len());
            let mut exposures = 0;
            let mut module_listed = 0;
            for (project, listed, members) in &meetings {
                if is_project && root != project.join(tools::skills::PROJECT_SKILLS) {
                    continue;
                }
                if listed.contains(&skill.name) {
                    exposures += 1;
                }
                if members.contains(&skill.name) {
                    module_listed += 1;
                }
            }
            let changes = unattributed(&root, &skill.name, since, usage.attributed(&skill.name));
            rows.push(SkillRow {
                root: root.display().to_string(),
                name: skill.name.clone(),
                origin: skill.origin,
                hidden: skill.hidden,
                bytes,
                exposures,
                module_listed,
                reads: usage.views(&skill.name, window),
                changes,
            });
        }
    }
    rows.sort_by(|left, right| (&left.root, &left.name).cmp(&(&right.root, &right.name)));
    rows
}

fn project_of(projects: &HashMap<String, PathBuf>, session_dir: &Path) -> Option<PathBuf> {
    let id = session_dir.file_name()?.to_str()?;
    let text = fs::read_to_string(session_dir.join("meta.json")).ok()?;
    let meta: SessionMeta = serde_json::from_str(&text).ok()?;
    if meta.id != id {
        return None;
    }
    projects.get(&meta.project_id).cloned()
}

/// The skill names a catalog lists and the modules it shows.
fn listed(catalog: &str) -> (HashSet<String>, HashSet<String>) {
    let mut names = HashSet::new();
    let mut modules = HashSet::new();
    for line in catalog.lines() {
        let Some(rest) = line.strip_prefix("- ") else {
            continue;
        };
        let Some((head, _)) = rest.split_once(": ") else {
            continue;
        };
        match head.split_once(" (") {
            Some((module, tail)) if tail.contains("skills)") => {
                modules.insert(module.to_string());
            }
            _ => {
                names.insert(head.to_string());
            }
        }
    }
    (names, modules)
}

fn module_members(
    modules: &HashSet<String>,
    project: &Path,
    maps: &HashMap<PathBuf, HashMap<String, Vec<String>>>,
) -> HashSet<String> {
    let Some(by_module) = maps.get(project) else {
        return HashSet::new();
    };
    modules
        .iter()
        .filter_map(|name| by_module.get(name))
        .flatten()
        .cloned()
        .collect()
}

/// One skills root's `.usage.jsonl`, grouped per skill name.
#[derive(Default)]
struct UsageLog {
    views: HashMap<String, Vec<u64>>,
    attributed: HashMap<String, u64>,
}

impl UsageLog {
    /// The whole log once per root: `views` in millis, the newest attributed change
    /// rounded up to the end of its whole second, like a file mtime would be.
    fn read(root: &Path) -> Self {
        let mut log = Self::default();
        let Ok(text) = fs::read_to_string(root.join(tools::skillstore::USAGE_LOG)) else {
            return log;
        };
        for entry in text
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        {
            let Some(name) = entry["name"].as_str() else {
                continue;
            };
            let millis = entry["time"].as_u64().unwrap_or(0).saturating_mul(1000);
            match entry["op"].as_str() {
                Some("view") => log.views.entry(name.to_string()).or_default().push(millis),
                Some("create" | "patch" | "write_file") => {
                    let slot = log.attributed.entry(name.to_string()).or_insert(0);
                    *slot = (*slot).max(millis.saturating_add(999));
                }
                _ => {}
            }
        }
        log
    }

    fn views(&self, name: &str, window: (u64, u64)) -> u64 {
        self.views.get(name).map_or(0, |times| {
            times
                .iter()
                .filter(|millis| **millis >= window.0 && **millis <= window.1)
                .count() as u64
        })
    }

    fn attributed(&self, name: &str) -> Option<u64> {
        self.attributed.get(name).copied()
    }
}

/// Skill files changed after the window opened and after the newest attributed change.
fn unattributed(root: &Path, name: &str, since: u64, attributed: Option<u64>) -> Vec<String> {
    let mut files = Vec::new();
    files_under(&root.join(name), &mut files);
    let mut changes = Vec::new();
    for file in files {
        let Ok(meta) = fs::metadata(&file) else {
            continue;
        };
        let Ok(modified) = meta.modified() else {
            continue;
        };
        let millis = modified
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_millis() as u64);
        if millis > since && attributed.is_none_or(|attributed| millis > attributed) {
            changes.push(format!("{} (mtime {millis})", file.display()));
        }
    }
    changes
}

fn files_under(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            files_under(&path, out);
        } else if path
            .file_name()
            .is_some_and(|name| name != tools::skillstore::USAGE_LOG)
        {
            out.push(path);
        }
    }
}

/// Replaces env-file values and secret shapes before anything is printed.
struct Redactor {
    values: Vec<String>,
    secret: Option<regex::Regex>,
}

impl Redactor {
    fn new(env_file: Option<&Path>) -> Self {
        let mut values: Vec<String> = Vec::new();
        if let Some(text) = env_file.and_then(|path| fs::read_to_string(path).ok()) {
            for line in text.lines() {
                let line = line.trim();
                if line.starts_with('#') {
                    continue;
                }
                let Some((_, value)) = line.split_once('=') else {
                    continue;
                };
                let value = value.trim().trim_matches(['"', '\'']);
                if value.len() >= 8 {
                    values.push(value.to_string());
                }
            }
        }
        let secret =
            regex::Regex::new(r"(sk-|sk_|ghp_|github_pat_|AKIA|Bearer |xox[bp]-)[A-Za-z0-9_./+-]+")
                .ok();
        Self { values, secret }
    }

    fn apply(&self, text: &str) -> String {
        let mut out = match &self.secret {
            Some(secret) => secret.replace_all(text, "[redacted]").to_string(),
            None => text.to_string(),
        };
        for value in &self.values {
            out = out.replace(value, "[redacted]");
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_exit_status_both_bash_spellings_show() {
        assert_eq!(exit_status("done\nexit code: 0\n"), Some(0));
        assert_eq!(exit_status("[exit status 3; 12 lines]\n"), Some(3));
        assert_eq!(exit_status("- exit code: 1"), Some(1));
        assert_eq!(exit_status("no status here"), None);
    }

    #[test]
    fn caps_prompts_tool_input_and_errors() {
        let long = "word ".repeat(200);
        let capped = cap(&long, MAX_PROMPT_CHARS);
        assert_eq!(capped.chars().count(), MAX_PROMPT_CHARS);
        assert!(capped.ends_with('…'));
        assert_eq!(cap("short", MAX_INPUT_CHARS), "short");
        assert_eq!(
            first_error_line("line one\nerror: boom\nerror: later").as_deref(),
            Some("error: boom")
        );
    }

    #[test]
    fn redacts_env_values_and_secret_shapes() {
        let redactor = Redactor {
            values: vec!["hunter2hunter2".to_string()],
            secret: Redactor::new(None).secret,
        };
        assert_eq!(redactor.apply("token hunter2hunter2"), "token [redacted]");
        assert_eq!(
            redactor.apply("use sk-abc123 and ghp_def456"),
            "use [redacted] and [redacted]"
        );
        assert!(redactor.apply("xoxb-99").contains("[redacted]"));
    }

    #[test]
    fn a_catalog_line_splits_names_from_modules() {
        let catalog = "- ember-flow: Route work.\n- media (2 skills): Audio. Use skill_search \"media\" to list.\n";
        let (names, modules) = listed(catalog);
        assert!(names.contains("ember-flow"));
        assert!(modules.contains("media"));
    }
}
