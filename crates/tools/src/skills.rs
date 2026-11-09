//! Skill discovery: `SKILL.md` folders under the global and the project skills roots.
//!
//! A skill is a directory whose `SKILL.md` opens with frontmatter naming it (`name`,
//! `description`, optional `metadata.origin`). A project skill wins over a global skill of the
//! same name. A malformed skill is skipped with a warning, so one bad file hides no other.

use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::skillstore;

/// Project skills live under this path, relative to the project root.
pub const PROJECT_SKILLS: &str = ".agents/skills";
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
/// skills come first, each group ordered by latest `view`. Empty when no skill is found.
pub fn catalog(intro: &str, global: Option<&Path>, project: &Path) -> String {
    let mut skills = discover(global, project);
    if skills.is_empty() {
        return String::new();
    }
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use super::{catalog, discover, valid_name, Skill};

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
        assert!(text.starts_with("INTRO"), "{text}");
        assert_eq!(catalog_names(&text).len(), 50);
        assert_eq!(text.lines().last(), Some("1 more skills: use skill_search"));
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
}
