//! Read-only views of agent-managed skills for the web: the list, one skill in full, and its
//! change history from the backups and the usage log. Writes stay in `skillmanage`.

use std::cmp::Reverse;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

use crate::skills::{self, Skill, PROJECT_SKILLS};
use crate::skillstore::{SUPPORT_DIRS, USAGE_LOG};

/// Backups and usage lines shown per skill; the newest go first.
const HISTORY_LIMIT: usize = 50;
/// Support files above this size are listed without their text.
const FILE_BYTES: u64 = 64 * 1024;

#[derive(Serialize)]
pub struct Summary {
    pub name: String,
    pub description: String,
    /// Found under a project root rather than the global root.
    pub project: bool,
    pub path: String,
}

#[derive(Serialize)]
pub struct SupportFile {
    pub path: String,
    /// `None` when the file is too large or not UTF-8 text.
    pub text: Option<String>,
}

#[derive(Serialize)]
pub struct Detail {
    #[serde(flatten)]
    pub summary: Summary,
    /// The full SKILL.md text, frontmatter included.
    pub text: String,
    pub files: Vec<SupportFile>,
}

#[derive(Serialize)]
pub struct Backup {
    pub at: u64,
    pub file: String,
    /// The file text before the change this backup records.
    pub text: String,
}

#[derive(Serialize)]
pub struct History {
    /// Newest first.
    pub backups: Vec<Backup>,
    /// `.usage.jsonl` lines for this skill, newest last.
    pub usage: Vec<Value>,
}

pub fn summary(skill: &Skill) -> Summary {
    Summary {
        name: skill.name.clone(),
        description: skill.description.clone(),
        project: skill.project,
        path: skill.path.display().to_string(),
    }
}

/// Agent-origin skills under each project root, then under the global root.
pub fn list(global: Option<&Path>, projects: &[PathBuf]) -> Vec<Skill> {
    let mut found = Vec::new();
    for root in projects {
        found.extend(skills::load(&root.join(PROJECT_SKILLS), true));
    }
    if let Some(root) = global {
        found.extend(skills::load(root, false));
    }
    found.retain(|skill| skill.origin == "agent");
    found
}

/// The skill's SKILL.md text and its supporting files under the support folders.
pub fn detail(skill: &Skill) -> Option<Detail> {
    let dir = skill.path.parent()?;
    let text = fs::read_to_string(&skill.path).ok()?;
    let mut files = Vec::new();
    for folder in SUPPORT_DIRS {
        let mut stack = vec![dir.join(folder)];
        while let Some(current) = stack.pop() {
            let Ok(entries) = fs::read_dir(&current) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                if kind.is_symlink() {
                    continue;
                } else if kind.is_dir() {
                    stack.push(path);
                } else if kind.is_file() {
                    let small = entry.metadata().is_ok_and(|meta| meta.len() <= FILE_BYTES);
                    let text = if small {
                        fs::read_to_string(&path).ok()
                    } else {
                        None
                    };
                    let rel = path.strip_prefix(dir).unwrap_or(&path);
                    files.push(SupportFile {
                        path: rel.display().to_string(),
                        text,
                    });
                }
            }
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Some(Detail {
        summary: summary(skill),
        text,
        files,
    })
}

/// Backups under `<root>/.backup/<name>` and this skill's lines of `<root>/.usage.jsonl`.
/// The root is the skills root the skill was read from, so each root keeps its own history.
pub fn history(skill: &Skill) -> History {
    let mut history = History {
        backups: Vec::new(),
        usage: Vec::new(),
    };
    let Some(dir) = skill.path.parent() else {
        return history;
    };
    let (Some(root), Some(name)) = (dir.parent(), dir.file_name().and_then(|n| n.to_str())) else {
        return history;
    };
    if let Ok(entries) = fs::read_dir(root.join(".backup").join(name)) {
        for entry in entries.flatten() {
            let file = entry.file_name().to_string_lossy().to_string();
            let Some((secs, rest)) = file.split_once('-') else {
                continue;
            };
            let (Ok(at), Ok(text)) = (secs.parse::<u64>(), fs::read_to_string(entry.path())) else {
                continue;
            };
            history.backups.push(Backup {
                at,
                file: rest.to_string(),
                text,
            });
        }
    }
    history.backups.sort_by_key(|backup| Reverse(backup.at));
    history.backups.truncate(HISTORY_LIMIT);

    let log = fs::read_to_string(root.join(USAGE_LOG)).unwrap_or_default();
    let mut usage: Vec<Value> = log
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|line| line["name"] == name)
        .collect();
    let skip = usage.len().saturating_sub(HISTORY_LIMIT);
    history.usage = usage.split_off(skip);
    history
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_skill(root: &Path, name: &str, origin: &str) {
        let dir = root.join(name);
        fs::create_dir_all(dir.join("references")).unwrap();
        let text = format!(
            "---\nname: {name}\ndescription: d\nmetadata:\n  origin: {origin}\n---\nbody\n"
        );
        fs::write(dir.join("SKILL.md"), text).unwrap();
        fs::write(dir.join("references/notes.md"), "note").unwrap();
    }

    #[test]
    fn lists_agent_skills_and_reads_detail_and_history() {
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("global");
        let project = tmp.path().join("proj");
        write_skill(&global, "deploy", "agent");
        write_skill(&global, "human-one", "human");
        write_skill(&project.join(PROJECT_SKILLS), "local", "agent");

        let found = list(Some(&global), std::slice::from_ref(&project));
        let names: Vec<&str> = found.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["local", "deploy"]);

        let deploy = found.iter().find(|s| s.name == "deploy").unwrap();
        let detail = detail(deploy).unwrap();
        assert!(detail.text.contains("body"));
        assert_eq!(detail.files.len(), 1);
        assert_eq!(detail.files[0].text.as_deref(), Some("note"));

        fs::create_dir_all(global.join(".backup/deploy")).unwrap();
        fs::write(global.join(".backup/deploy/100-SKILL.md"), "old").unwrap();
        fs::write(global.join(".backup/deploy/200-SKILL.md"), "older").unwrap();
        fs::write(
            global.join(USAGE_LOG),
            "{\"name\":\"deploy\",\"op\":\"patch\",\"result\":\"ok\",\"time\":1}\n{\"name\":\"other\",\"op\":\"create\",\"result\":\"ok\",\"time\":2}\n",
        )
        .unwrap();
        let view = history(deploy);
        let stamps: Vec<u64> = view.backups.iter().map(|b| b.at).collect();
        assert_eq!(stamps, vec![200, 100]);
        assert_eq!(view.usage.len(), 1);
        assert_eq!(view.usage[0]["op"], "patch");
    }
}
