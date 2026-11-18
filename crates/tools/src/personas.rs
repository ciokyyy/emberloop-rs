//! Persona discovery: `*.md` files whose frontmatter `name` is the persona id and whose body is
//! the system prompt of a goldfish Session.
//!
//! Search order, first match wins, so a project persona always shadows a global one:
//! 1. `<project>/.agents/agents/*.md`
//! 2. `agents/*.md` inside each skill folder under `<project>/.agents/skills`
//! 3. the `agents` directory beside the global skills root
//! 4. `agents/*.md` inside each skill folder under the global skills root
//!
//! Files are read on every call; there is no cache. A file without a valid `name` is skipped with
//! a warning, so one bad file hides no other. Other frontmatter keys are ignored.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::skills::{valid_name, PROJECT_SKILLS};

/// Standalone project personas live under this path, relative to the project root.
pub const PROJECT_AGENTS: &str = ".agents/agents";
/// A skill folder bundles its personas under this subfolder.
const BUNDLED_AGENTS: &str = "agents";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Persona {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
    /// The file text after the frontmatter: the system prompt.
    pub body: String,
}

/// Every persona, in search order; a later persona of an already seen name is dropped.
pub fn discover(global_skills: Option<&Path>, project: &Path) -> Vec<Persona> {
    let mut dirs = vec![project.join(PROJECT_AGENTS)];
    dirs.extend(bundled_dirs(&project.join(PROJECT_SKILLS)));
    if let Some(global) = global_skills {
        if let Some(parent) = global.parent() {
            dirs.push(parent.join(BUNDLED_AGENTS));
        }
        dirs.extend(bundled_dirs(global));
    }
    let mut seen = HashSet::new();
    dirs.iter()
        .flat_map(|dir| load_dir(dir))
        .filter(|persona| seen.insert(persona.name.clone()))
        .collect()
}

/// The system prompt of persona `name`, or `None` when no search level holds it.
pub fn resolve(global_skills: Option<&Path>, project: &Path, name: &str) -> Option<String> {
    discover(global_skills, project)
        .into_iter()
        .find(|persona| persona.name == name)
        .map(|persona| persona.body)
}

/// `<skill folder>/agents` for each skill folder under `root`, ordered by folder name.
fn bundled_dirs(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path().join(BUNDLED_AGENTS))
        .filter(|dir| dir.is_dir())
        .collect();
    dirs.sort();
    dirs
}

/// The valid personas among the `*.md` files directly in `dir`, ordered by file name.
fn load_dir(dir: &Path) -> Vec<Persona> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "md") && path.is_file())
        .collect();
    files.sort();
    files.into_iter().filter_map(load_one).collect()
}

fn load_one(path: PathBuf) -> Option<Persona> {
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) => {
            tracing::warn!(path = %path.display(), %error, "skipping unreadable persona");
            return None;
        }
    };
    let Some((name, description, body)) = parse(&text) else {
        tracing::warn!(path = %path.display(), "skipping persona: frontmatter needs a valid name");
        return None;
    };
    Some(Persona {
        name,
        description,
        path,
        body,
    })
}

/// `(name, description, body)` from the frontmatter; the description may be empty.
fn parse(text: &str) -> Option<(String, String, String)> {
    let rest = text.strip_prefix("---\n")?;
    let (head, body) = rest.split_once("\n---")?;
    let mut name = None;
    let mut description = String::new();
    for line in head.lines() {
        if let Some(value) = line.strip_prefix("name:") {
            name = Some(scalar(value));
        } else if let Some(value) = line.strip_prefix("description:") {
            description = scalar(value);
        }
    }
    let name = name.filter(|name| valid_name(name))?;
    let body = body.strip_prefix('\n').unwrap_or(body).to_string();
    Some((name, description, body))
}

fn scalar(raw: &str) -> String {
    raw.trim().trim_matches(['"', '\'']).to_string()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::{discover, resolve};

    fn persona(dir: &Path, file: &str, front: &str, body: &str) {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join(file), format!("---\n{front}\n---\n{body}\n")).unwrap();
    }

    #[test]
    fn four_levels_resolve_and_the_earlier_level_wins() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        let home = dir.path().join("home");
        let global = home.join("skills");
        let all = |level: &str| format!("name: p-{level}\ndescription: d");
        persona(&project.join(".agents/agents"), "a.md", &all("1"), "one");
        persona(
            &project.join(".agents/skills/s/agents"),
            "a.md",
            &all("2"),
            "two",
        );
        persona(&home.join("agents"), "a.md", &all("3"), "three");
        persona(&global.join("s/agents"), "a.md", &all("4"), "four");
        let found = discover(Some(&global), &project);
        let names: Vec<&str> = found.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["p-1", "p-2", "p-3", "p-4"]);
        for (level, body) in [("1", "one"), ("2", "two"), ("3", "three"), ("4", "four")] {
            let text = resolve(Some(&global), &project, &format!("p-{level}")).unwrap();
            assert_eq!(text.trim(), body);
        }
        assert_eq!(resolve(Some(&global), &project, "missing"), None);
    }

    #[test]
    fn project_bundled_beats_global_standalone() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        let home = dir.path().join("home");
        let front = "name: same\ndescription: d\ntools: Read";
        persona(
            &project.join(".agents/skills/s/agents"),
            "x.md",
            front,
            "project",
        );
        persona(&home.join("agents"), "x.md", front, "global");
        let text = resolve(Some(&home.join("skills")), &project, "same").unwrap();
        assert_eq!(text.trim(), "project");
    }

    #[test]
    fn id_is_the_frontmatter_name_and_bad_frontmatter_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let agents = dir.path().join(".agents/agents");
        persona(
            &agents,
            "file-name.md",
            "name: real-id\ndescription: d",
            "ok",
        );
        persona(&agents, "no-name.md", "description: d", "x");
        persona(
            &agents,
            "bad-name.md",
            "name: Bad Name!\ndescription: d",
            "x",
        );
        fs::write(agents.join("no-front.md"), "just text").unwrap();
        fs::write(agents.join("notes.txt"), "ignored").unwrap();
        let found = discover(None, dir.path());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "real-id");
        assert_eq!(found[0].path, agents.join("file-name.md"));
    }
}
