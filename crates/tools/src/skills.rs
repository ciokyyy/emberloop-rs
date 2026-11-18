//! Skill discovery: `SKILL.md` folders under the global and the project skills roots.
//!
//! A skill is a directory whose `SKILL.md` opens with frontmatter naming it (`name`,
//! `description`, optional `metadata.origin`). A project skill wins over a global skill of the
//! same name. A malformed skill is skipped with a warning, so one bad file hides no other.

use std::cmp::Reverse;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::skillstore;

/// Project skills live under this path, relative to the project root.
pub const PROJECT_SKILLS: &str = ".agents/skills";
/// Groups skill folders into modules, in each skills root.
pub const MODULES_FILE: &str = "skill-modules.json";
/// Opening bytes of a skill body that `skill_search` indexes; the rest is not searched.
const SEARCH_BODY_BYTES: usize = 2048;
/// Descriptions show at most this many characters in a catalog line or a search hit.
pub const DESCRIPTION_CHARS: usize = 150;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    /// `agent` or `downloaded`; `human` when the frontmatter names no origin.
    pub origin: String,
    /// The SKILL.md this skill was read from.
    pub path: PathBuf,
    /// Found under the project root rather than the global one.
    pub project: bool,
    /// The file text after the frontmatter.
    pub body: String,
}

impl Skill {
    /// What `skill_search` ranks: name, description, body headings and the opening body bytes.
    pub fn search_text(&self) -> String {
        let headings: Vec<&str> = self
            .body
            .lines()
            .filter(|line| line.starts_with('#'))
            .collect();
        format!(
            "{} {}\n{}\n{}",
            self.name,
            self.description,
            headings.join("\n"),
            clip_bytes(&self.body, SEARCH_BODY_BYTES)
        )
    }
}

/// Skills under the global root and under `<project>/.agents/skills`: project first, each group
/// sorted by name. A project skill hides a global skill of the same name.
pub fn discover(global: Option<&Path>, project: &Path) -> Vec<Skill> {
    let mut skills = load(&project.join(PROJECT_SKILLS), true);
    let taken: HashSet<String> = skills.iter().map(|skill| skill.name.clone()).collect();
    for skill in global.map(|root| load(root, false)).unwrap_or_default() {
        if taken.contains(&skill.name) {
            tracing::warn!(path = %skill.path.display(), "global skill shadowed by a project skill");
        } else {
            skills.push(skill);
        }
    }
    skills
}

pub fn load(root: &Path, project: bool) -> Vec<Skill> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut skills: Vec<Skill> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|dir| dir.is_dir())
        .filter_map(|dir| load_one(&dir, project))
        .collect();
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    skills
}

/// Finds a single skill by name: project skill first, then global.
pub fn locate(global: Option<&Path>, project: &Path, name: &str) -> Option<Skill> {
    if !valid_name(name) {
        return None;
    }
    let project_dir = project.join(PROJECT_SKILLS).join(name);
    if let Some(skill) = load_one(&project_dir, true) {
        return Some(skill);
    }
    if let Some(global_root) = global {
        let global_dir = global_root.join(name);
        if let Some(skill) = load_one(&global_dir, false) {
            return Some(skill);
        }
    }
    None
}

fn load_one(dir: &Path, project: bool) -> Option<Skill> {
    let path = dir.join("SKILL.md");
    if !path.is_file() {
        return None;
    }
    let text = read_skill(&path)?;
    let front = checked_front(dir, &path, &text)?;
    Some(Skill {
        name: front.name,
        description: front.description,
        origin: front.origin,
        path,
        project,
        body: front.body,
    })
}

/// The SKILL.md text, or a warning when it cannot be read.
fn read_skill(path: &Path) -> Option<String> {
    match fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(error) => {
            tracing::warn!(path = %path.display(), %error, "skipping unreadable skill");
            None
        }
    }
}

/// The frontmatter when it is valid and names the folder it sits in; otherwise a warning.
fn checked_front(dir: &Path, path: &Path, text: &str) -> Option<Front> {
    let Some(front) = parse(text) else {
        tracing::warn!(path = %path.display(), "skipping skill: frontmatter needs name and description");
        return None;
    };
    if dir.file_name().and_then(|name| name.to_str()) != Some(front.name.as_str()) {
        tracing::warn!(path = %path.display(), "skipping skill: folder name differs from its name");
        return None;
    }
    Some(front)
}

pub(crate) struct Front {
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) origin: String,
    pub(crate) body: String,
}

/// Reads the frontmatter: `name:`, `description:` and the nested `metadata:` `origin:`.
pub(crate) fn parse(text: &str) -> Option<Front> {
    let rest = text.strip_prefix("---\n")?;
    let (head, body) = rest.split_once("\n---")?;
    let mut name = None;
    let mut description = None;
    let mut origin = None;
    for line in head.lines() {
        if let Some(value) = line.strip_prefix("name:") {
            name = Some(scalar(value));
        } else if let Some(value) = line.strip_prefix("description:") {
            description = Some(scalar(value));
        } else if let Some(value) = line.strip_prefix("  origin:") {
            origin = Some(scalar(value));
        }
    }
    let name = name.filter(|name| valid_name(name))?;
    let description = description.filter(|text| !text.is_empty())?;
    let origin = origin
        .filter(|origin| !origin.is_empty())
        .unwrap_or_else(|| "human".to_string());
    Some(Front {
        name,
        description,
        origin,
        body: body.strip_prefix('\n').unwrap_or(body).to_string(),
    })
}

fn scalar(raw: &str) -> String {
    raw.trim().trim_matches(['"', '\'']).to_string()
}

/// Skill names: `[a-z0-9][a-z0-9._-]*`, at most 64 bytes.
pub fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    name.len() <= 64
        && chars
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && chars
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'))
}

/// The first `max` bytes of `text`, cut back to a character boundary.
fn clip_bytes(text: &str, max: usize) -> &str {
    let mut end = max.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// `text` with `metadata.origin: agent` in its frontmatter, in place of any other origin.
pub(crate) fn with_agent_origin(text: &str) -> Option<String> {
    let rest = text.strip_prefix("---\n")?;
    let (head, body) = rest.split_once("\n---")?;
    let mut lines: Vec<&str> = head
        .lines()
        .filter(|line| !line.starts_with("  origin:"))
        .collect();
    match lines.iter().position(|line| *line == "metadata:") {
        Some(at) => lines.insert(at + 1, "  origin: agent"),
        None => lines.extend(["metadata:", "  origin: agent"]),
    }
    Some(format!("---\n{}\n---{body}", lines.join("\n")))
}

/// Skill lines the catalog lists before it names the rest.
const CATALOG_LINES: usize = 50;

/// The catalog a Session freezes: `intro`, then one `- name: description` line per skill. Project
/// skills come first, each group ordered by latest `view`. A skill in a module is not listed
/// individually; each module with a member follows the list as one line. Empty when no skill is
/// found.
pub fn catalog(intro: &str, global: Option<&Path>, project: &Path) -> String {
    let mut skills = discover(global, project);
    if skills.is_empty() {
        return String::new();
    }
    let modules = modules(global, project);
    let grouped: HashSet<&str> = modules
        .iter()
        .flat_map(|module| module.skills.iter().map(String::as_str))
        .collect();
    skills.retain(|skill| !grouped.contains(skill.name.as_str()));
    let project_views = last_views(&project.join(PROJECT_SKILLS));
    let global_views = global.map(last_views).unwrap_or_default();
    skills.sort_by_key(|skill| {
        let views = if skill.project {
            &project_views
        } else {
            &global_views
        };
        (
            !skill.project,
            Reverse(views.get(&skill.name).copied().unwrap_or(0)),
        )
    });
    let mut out = format!("{}\n\n", intro.trim_end());
    for skill in skills.iter().take(CATALOG_LINES) {
        let description: String = skill.description.chars().take(DESCRIPTION_CHARS).collect();
        out.push_str(&format!("- {}: {description}\n", skill.name));
    }
    if skills.len() > CATALOG_LINES {
        out.push_str(&format!(
            "{} more skills: use skill_search\n",
            skills.len() - CATALOG_LINES
        ));
    }
    for module in modules.iter().filter(|module| !module.skills.is_empty()) {
        let about: String = module.about.chars().take(DESCRIPTION_CHARS).collect();
        out.push_str(&format!(
            "- {} ({} skills): {about}. Use skill_search \"{}\" to list.\n",
            module.name,
            module.skills.len(),
            module.name
        ));
    }
    out
}

/// The index `read` answers `el://skills` with: a line per module, then the names of the skills no
/// module holds. It lists catalogues, never skill bodies, so the answer stays small enough for a
/// harness to pass on whole.
pub fn index(global: Option<&Path>, project: &Path) -> String {
    let skills = discover(global, project);
    if skills.is_empty() {
        return String::new();
    }
    let modules: Vec<Module> = modules(global, project)
        .into_iter()
        .filter(|module| !module.skills.is_empty())
        .collect();
    let grouped: HashSet<&str> = modules
        .iter()
        .flat_map(|module| module.skills.iter().map(String::as_str))
        .collect();
    let loose: Vec<&str> = skills
        .iter()
        .filter(|skill| !grouped.contains(skill.name.as_str()))
        .map(|skill| skill.name.as_str())
        .collect();
    let mut out = format!(
        "Skills: {}. Modules: {}. Read one with el://skills/<name>; skill_search lists and ranks them.\n",
        skills.len(),
        modules.len()
    );
    for module in &modules {
        let about: String = module.about.chars().take(DESCRIPTION_CHARS).collect();
        out.push_str(&format!(
            "\n- {} ({} skills): {about}",
            module.name,
            module.skills.len()
        ));
    }
    if !loose.is_empty() {
        out.push_str(&format!(
            "\n- ungrouped ({}): {}",
            loose.len(),
            loose.join(", ")
        ));
    }
    out.push('\n');
    out
}

/// The latest `view` time per skill name in one skills root's usage log.
fn last_views(root: &Path) -> HashMap<String, u64> {
    let mut views: HashMap<String, u64> = HashMap::new();
    let Ok(log) = fs::read_to_string(root.join(skillstore::USAGE_LOG)) else {
        return views;
    };
    for entry in log
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
    {
        if entry["op"].as_str() != Some("view") {
            continue;
        }
        let (Some(name), Some(time)) = (entry["name"].as_str(), entry["time"].as_u64()) else {
            continue;
        };
        let latest = views.entry(name.to_string()).or_default();
        *latest = (*latest).max(time);
    }
    views
}

/// A group of skills the catalog lists as one line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Module {
    pub name: String,
    pub about: String,
    /// The named skills that exist, in merge order.
    pub skills: Vec<String>,
}

/// The modules of both skills roots, sorted by name. A module named in both roots merges: the
/// project's `about`, then both member lists, global first. A member that names no skill, or a
/// skill an earlier module already holds, is dropped.
pub fn modules(global: Option<&Path>, project: &Path) -> Vec<Module> {
    let existing: HashSet<String> = discover(global, project)
        .into_iter()
        .map(|skill| skill.name)
        .collect();
    let mut modules = read_modules(&project.join(PROJECT_SKILLS).join(MODULES_FILE));
    let mut at: HashMap<String, usize> = modules
        .iter()
        .enumerate()
        .map(|(index, module)| (module.name.clone(), index))
        .collect();
    for module in global
        .map(|root| read_modules(&root.join(MODULES_FILE)))
        .unwrap_or_default()
    {
        match at.get(&module.name) {
            Some(&index) => {
                let mut skills = module.skills;
                skills.append(&mut modules[index].skills);
                modules[index].skills = skills;
            }
            None => {
                at.insert(module.name.clone(), modules.len());
                modules.push(module);
            }
        }
    }
    modules.sort_by(|a, b| a.name.cmp(&b.name));
    let mut claimed: HashSet<String> = HashSet::new();
    for module in &mut modules {
        let name = module.name.clone();
        module
            .skills
            .retain(|skill| claim(&name, skill, &existing, &mut claimed));
    }
    modules
}

/// True when `skill` exists and no earlier module or earlier entry has claimed it.
fn claim(
    module: &str,
    skill: &str,
    existing: &HashSet<String>,
    claimed: &mut HashSet<String>,
) -> bool {
    let keep = existing.contains(skill) && claimed.insert(skill.to_string());
    if !keep {
        tracing::warn!(module, %skill, "skill module drops a missing or already claimed skill");
    }
    keep
}

/// The module `skill` belongs to, when one names it.
pub fn module_of<'a>(modules: &'a [Module], skill: &str) -> Option<&'a str> {
    modules
        .iter()
        .find(|module| module.skills.iter().any(|member| member == skill))
        .map(|module| module.name.as_str())
}

/// One root's modules, or a warning when the file is there but unreadable as modules.
fn read_modules(path: &Path) -> Vec<Module> {
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    match serde_json::from_str::<BTreeMap<String, ModuleEntry>>(&text) {
        Ok(entries) => entries
            .into_iter()
            .map(|(name, entry)| Module {
                name,
                about: entry.about,
                skills: entry.skills,
            })
            .collect(),
        Err(error) => {
            tracing::warn!(path = %path.display(), %error, "skipping malformed skill modules file");
            Vec::new()
        }
    }
}

#[derive(serde::Deserialize)]
struct ModuleEntry {
    about: String,
    skills: Vec<String>,
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use super::{catalog, discover, index, module_of, modules, valid_name, Skill};

    fn usage(root: &Path, name: &str, time: u64) {
        fs::create_dir_all(root).unwrap();
        let line =
            format!("{{\"time\":{time},\"name\":\"{name}\",\"op\":\"view\",\"result\":\"ok\"}}\n");
        let mut text = fs::read_to_string(root.join(".usage.jsonl")).unwrap_or_default();
        text.push_str(&line);
        fs::write(root.join(".usage.jsonl"), text).unwrap();
    }

    fn catalog_names(text: &str) -> Vec<String> {
        text.lines()
            .filter_map(|line| line.strip_prefix("- "))
            .map(|line| line.split(':').next().unwrap_or_default().to_string())
            .collect()
    }

    #[test]
    fn catalog_lists_fifty_skills_then_names_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let global = dir.path().join("global");
        for n in 0..51 {
            let name = format!("skill-{n:02}");
            skill(&global, &name, &frontmatter(&name, "short"));
        }
        let text = catalog("INTRO", Some(&global), dir.path());
        assert_eq!(catalog_names(&text).len(), 50);
        assert_eq!(text.lines().last(), Some("1 more skills: use skill_search"));
    }

    #[test]
    fn index_lists_modules_and_only_the_skills_no_module_holds() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        let root = project.join(".agents/skills");
        skill(&root, "grouped", &frontmatter("grouped", "in a module"));
        skill(&root, "solo", &frontmatter("solo", "by itself"));
        fs::write(
            root.join("skill-modules.json"),
            r#"{"tools": {"about": "Use when tooling.", "skills": ["grouped"]}}"#,
        )
        .unwrap();

        let text = index(None, &project);
        assert!(
            text.contains("- tools (1 skills): Use when tooling."),
            "{text}"
        );
        assert!(text.contains("- ungrouped (1): solo"), "{text}");
        assert!(
            !text.contains("- grouped"),
            "a module member is not listed alone: {text}"
        );
        assert!(
            !text.contains("by itself"),
            "the index carries no descriptions: {text}"
        );
    }

    #[test]
    fn catalog_clips_each_description_to_150_characters() {
        let dir = tempfile::tempdir().unwrap();
        let global = dir.path().join("global");
        skill(&global, "wide", &frontmatter("wide", &"d".repeat(300)));
        let text = catalog("INTRO", Some(&global), dir.path());
        let line = text
            .lines()
            .find(|line| line.starts_with("- wide: "))
            .unwrap();
        assert_eq!(line.chars().count(), "- wide: ".chars().count() + 150);
    }

    #[test]
    fn catalog_lists_project_first_then_by_most_recent_view() {
        let dir = tempfile::tempdir().unwrap();
        let global = dir.path().join("global");
        let project = dir.path().join("project");
        for name in ["a", "b", "c"] {
            skill(&global, name, &frontmatter(name, "global"));
        }
        skill(
            &project.join(".agents/skills"),
            "p",
            &frontmatter("p", "project"),
        );
        usage(&global, "a", 5);
        usage(&global, "c", 9);
        let text = catalog("INTRO", Some(&global), &project);
        assert_eq!(catalog_names(&text), vec!["p", "c", "a", "b"]);
    }

    #[test]
    fn catalog_is_empty_when_there_are_no_skills() {
        let dir = tempfile::tempdir().unwrap();
        assert!(catalog("INTRO", None, dir.path()).is_empty());
    }

    fn skill(root: &Path, dir: &str, text: &str) {
        let folder = root.join(dir);
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("SKILL.md"), text).unwrap();
    }

    fn frontmatter(name: &str, description: &str) -> String {
        format!("---\nname: {name}\ndescription: {description}\n---\n# {name}\n\nBody text.\n")
    }

    #[test]
    fn discover_reads_both_roots_and_project_wins_on_collision() {
        let dir = tempfile::tempdir().unwrap();
        let global = dir.path().join("global");
        let project = dir.path().join("project");
        let project_skills = project.join(".agents/skills");
        skill(&global, "shared", &frontmatter("shared", "global version"));
        skill(
            &global,
            "only-global",
            &frontmatter("only-global", "from global"),
        );
        skill(
            &project_skills,
            "shared",
            &frontmatter("shared", "project version"),
        );

        let found = discover(Some(&global), &project);
        let seen: Vec<(&str, &str, bool)> = found
            .iter()
            .map(|s| (s.name.as_str(), s.description.as_str(), s.project))
            .collect();
        assert_eq!(
            seen,
            vec![
                ("shared", "project version", true),
                ("only-global", "from global", false),
            ]
        );
        assert!(found[0]
            .path
            .ends_with("project/.agents/skills/shared/SKILL.md"));
    }

    #[test]
    fn origin_is_read_from_metadata_and_defaults_to_human() {
        let dir = tempfile::tempdir().unwrap();
        let global = dir.path().join("global");
        skill(
            &global,
            "made",
            "---\nname: made\ndescription: by agent\nmetadata:\n  origin: agent\n---\nbody\n",
        );
        skill(&global, "hand", &frontmatter("hand", "by hand"));

        let origins: Vec<String> = discover(Some(&global), dir.path())
            .into_iter()
            .map(|s| s.origin)
            .collect();
        assert_eq!(origins, vec!["human", "agent"]);
    }

    #[test]
    fn symlinked_skill_folders_are_discovered() {
        let dir = tempfile::tempdir().unwrap();
        let global = dir.path().join("global");
        let elsewhere = dir.path().join("elsewhere");
        skill(
            &elsewhere,
            "linked",
            &frontmatter("linked", "behind a link"),
        );
        fs::create_dir_all(&global).unwrap();
        std::os::unix::fs::symlink(elsewhere.join("linked"), global.join("linked")).unwrap();

        let found = discover(Some(&global), dir.path());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].description, "behind a link");
    }

    #[test]
    fn malformed_skills_are_skipped_and_the_rest_still_load() {
        let dir = tempfile::tempdir().unwrap();
        let global = dir.path().join("global");
        skill(&global, "good", &frontmatter("good", "fine"));
        skill(&global, "no-frontmatter", "# just a heading\n");
        skill(
            &global,
            "no-description",
            "---\nname: no-description\n---\nbody\n",
        );
        skill(
            &global,
            "Bad_Name",
            &frontmatter("Bad_Name", "upper case name"),
        );
        skill(
            &global,
            "renamed",
            &frontmatter("other", "folder and name differ"),
        );

        let names: Vec<String> = discover(Some(&global), dir.path())
            .into_iter()
            .map(|s| s.name)
            .collect();
        assert_eq!(names, vec!["good"]);
    }

    #[test]
    fn names_follow_the_skill_name_rule() {
        assert!(valid_name("a"));
        assert!(valid_name("pdf-tools.v2_x"));
        assert!(valid_name(&"a".repeat(64)));
        assert!(!valid_name(""));
        assert!(!valid_name("-leading"));
        assert!(!valid_name("Upper"));
        assert!(!valid_name("has space"));
        assert!(!valid_name("../escape"));
        assert!(!valid_name(&"a".repeat(65)));
    }

    #[test]
    fn search_text_reads_name_description_headings_and_the_first_two_kilobytes() {
        let skill = Skill {
            name: "n".into(),
            description: "d".into(),
            origin: "human".into(),
            path: PathBuf::new(),
            project: false,
            body: format!("# Heading\n{}tail-marker", "x".repeat(3000)),
        };
        let text = skill.search_text();
        assert!(text.contains("Heading"), "{text}");
        assert!(!text.contains("tail-marker"), "{text}");
    }

    fn modules_file(root: &Path, json: &str) {
        fs::create_dir_all(root).unwrap();
        fs::write(root.join("skill-modules.json"), json).unwrap();
    }

    #[test]
    fn catalog_collapses_a_module_into_one_line() {
        let dir = tempfile::tempdir().unwrap();
        let global = dir.path().join("global");
        for name in ["audio", "plain", "video"] {
            skill(&global, name, &frontmatter(name, "short"));
        }
        modules_file(
            &global,
            r#"{"media": {"about": "audio and video", "skills": ["audio", "video"]}}"#,
        );

        let text = catalog("INTRO", Some(&global), dir.path());
        assert!(text.contains("- plain: short\n"), "{text}");
        assert!(!text.contains("- audio:"), "{text}");
        let line = "- media (2 skills): audio and video. Use skill_search \"media\" to list.";
        assert!(text.contains(line), "{text}");
        let plain = text
            .lines()
            .position(|line| line == "- plain: short")
            .unwrap();
        let media = text
            .lines()
            .position(|line| line.starts_with("- media "))
            .unwrap();
        assert!(plain < media, "{text}");
    }

    #[test]
    fn a_module_named_in_both_roots_merges_with_the_project_about() {
        let dir = tempfile::tempdir().unwrap();
        let global = dir.path().join("global");
        let project = dir.path().join("project");
        for name in ["both", "ga", "pa"] {
            skill(&global, name, &frontmatter(name, "short"));
        }
        skill(
            &project.join(".agents/skills"),
            "pa",
            &frontmatter("pa", "short"),
        );
        modules_file(
            &global,
            r#"{"media": {"about": "global media", "skills": ["ga", "both"]}}"#,
        );
        modules_file(
            &project.join(".agents/skills"),
            r#"{"media": {"about": "project media", "skills": ["both", "pa"]}, "alpha": {"about": "a", "skills": []}}"#,
        );

        let found = modules(Some(&global), &project);
        let names: Vec<&str> = found.iter().map(|module| module.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "media"]);
        assert_eq!(found[1].about, "project media");
        assert_eq!(found[1].skills, vec!["ga", "both", "pa"]);

        let text = catalog("INTRO", Some(&global), &project);
        assert!(!text.contains("- ga:"), "{text}");
        assert!(!text.contains("- pa:"), "{text}");
        assert!(
            text.contains("- media (3 skills): project media."),
            "{text}"
        );
    }

    #[test]
    fn missing_module_file_leaves_every_skill_unmapped() {
        let dir = tempfile::tempdir().unwrap();
        let global = dir.path().join("global");
        skill(&global, "audio", &frontmatter("audio", "short"));

        assert!(modules(Some(&global), dir.path()).is_empty());
        assert_eq!(
            catalog_names(&catalog("INTRO", Some(&global), dir.path())),
            vec!["audio"]
        );
    }

    #[test]
    fn malformed_module_file_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let global = dir.path().join("global");
        skill(&global, "audio", &frontmatter("audio", "short"));
        modules_file(&global, r#"{"media": {"about": 1}}"#);

        assert!(modules(Some(&global), dir.path()).is_empty());
        assert_eq!(
            catalog_names(&catalog("INTRO", Some(&global), dir.path())),
            vec!["audio"]
        );
    }

    #[test]
    fn a_module_member_without_a_skill_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let global = dir.path().join("global");
        skill(&global, "audio", &frontmatter("audio", "short"));
        modules_file(
            &global,
            r#"{"media": {"about": "a", "skills": ["audio", "ghost"]}}"#,
        );

        let found = modules(Some(&global), dir.path());
        assert_eq!(found[0].skills, vec!["audio"]);
        let text = catalog("INTRO", Some(&global), dir.path());
        assert!(text.contains("- media (1 skills): a."), "{text}");

        modules_file(
            &global,
            r#"{"ghostly": {"about": "g", "skills": ["ghost"]}}"#,
        );
        let text = catalog("INTRO", Some(&global), dir.path());
        assert!(!text.contains("ghostly"), "{text}");
        assert_eq!(catalog_names(&text), vec!["audio"]);
    }

    #[test]
    fn a_skill_belongs_to_the_first_module_that_names_it() {
        let dir = tempfile::tempdir().unwrap();
        let global = dir.path().join("global");
        skill(&global, "shared", &frontmatter("shared", "short"));
        modules_file(
            &global,
            r#"{"beta": {"about": "b", "skills": ["shared"]}, "alpha": {"about": "a", "skills": ["shared"]}}"#,
        );

        let found = modules(Some(&global), dir.path());
        let names: Vec<&str> = found.iter().map(|module| module.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "beta"]);
        assert_eq!(found[0].skills, vec!["shared"]);
        assert!(found[1].skills.is_empty());
        assert_eq!(module_of(&found, "shared"), Some("alpha"));
        assert_eq!(module_of(&found, "other"), None);

        let text = catalog("INTRO", Some(&global), dir.path());
        assert!(text.contains("- alpha (1 skills): a."), "{text}");
        assert!(!text.contains("- beta ("), "{text}");
    }

    #[test]
    fn a_skill_in_modules_of_both_roots_goes_to_the_first_module() {
        let dir = tempfile::tempdir().unwrap();
        let global = dir.path().join("global");
        let project = dir.path().join("project");
        skill(&global, "shared", &frontmatter("shared", "short"));
        modules_file(&global, r#"{"beta": {"about": "b", "skills": ["shared"]}}"#);
        modules_file(
            &project.join(".agents/skills"),
            r#"{"alpha": {"about": "a", "skills": ["shared"]}}"#,
        );

        let found = modules(Some(&global), &project);
        assert_eq!(module_of(&found, "shared"), Some("alpha"));
        assert!(found[1].skills.is_empty());
    }
}
