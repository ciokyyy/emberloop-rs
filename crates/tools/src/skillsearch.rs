//! `skill_search`: BM25 over every discovered skill, global and project alike.

use serde::Deserialize;
use serde_json::Value;

use crate::skills::{self, DESCRIPTION_CHARS};
use crate::{bm25, ToolError, ToolResult, Toolbox};

const DEFAULT_LIMIT: usize = 5;
const MAX_LIMIT: usize = 20;

#[derive(Deserialize)]
struct SkillArgs {
    query: String,
    #[serde(default)]
    limit: Option<usize>,
}

/// Ranked skills for `query`, each with its origin, clipped description and SKILL.md path.
pub fn run(toolbox: &Toolbox, args: &Value) -> ToolResult<String> {
    let SkillArgs { query, limit } = serde_json::from_value(args.clone())?;
    let query = query.trim();
    if query.is_empty() {
        return Err(ToolError::msg("query is required: give a search string"));
    }
    let limit = limit
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_LIMIT)
        .min(MAX_LIMIT);
    let found = skills::discover(toolbox.global_skills(), toolbox.root());
    let docs: Vec<Vec<String>> = found
        .iter()
        .map(|skill| bm25::tokens(&skill.search_text()))
        .collect();
    let scores = bm25::scores(&docs, &bm25::query_terms(query));
    let mut hits: Vec<(f64, &skills::Skill)> = scores
        .into_iter()
        .zip(&found)
        .filter(|(score, _)| *score > 0.0)
        .collect();
    hits.sort_by(|a, b| b.0.total_cmp(&a.0));
    hits.truncate(limit);
    if hits.is_empty() {
        return Ok(format!("query {query:?}: nothing matched\n"));
    }
    let mut out = format!("query {query:?}: {} match(es)\n", hits.len());
    for (_, skill) in hits {
        let description: String = skill.description.chars().take(DESCRIPTION_CHARS).collect();
        out.push_str(&format!(
            "- {} [{}]: {description}\n  {}\n",
            skill.name,
            skill.origin,
            skill.path.display()
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use serde_json::json;

    use super::run;
    use crate::{ToolConfig, Toolbox};

    fn write_skill(root: &Path, name: &str, description: &str, body: &str) {
        let folder = root.join(name);
        fs::create_dir_all(&folder).unwrap();
        let text = format!("---\nname: {name}\ndescription: {description}\n---\n{body}\n");
        fs::write(folder.join("SKILL.md"), text).unwrap();
    }

    /// Returns the tempdir (kept alive by the caller), the global skills root and the project root.
    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let base = fs::canonicalize(dir.path()).unwrap();
        let global = base.join("global");
        let project = base.join("project");
        fs::create_dir_all(&global).unwrap();
        fs::create_dir_all(&project).unwrap();
        (dir, global, project)
    }

    fn toolbox(global: &Path, project: &Path) -> Toolbox {
        let spill = project.parent().unwrap().join("spill");
        Toolbox::new(ToolConfig {
            global_skills: Some(global.to_path_buf()),
            ..ToolConfig::new(project, spill)
        })
        .unwrap()
    }

    #[test]
    fn rare_term_skill_ranks_above_common_term_skill() {
        let (_dir, global, project) = fixture();
        write_skill(
            &global,
            "alpha",
            "general helper",
            &"common words\n".repeat(20),
        );
        write_skill(&global, "beta", "kubernetes rollout", "common words");
        write_skill(&global, "gamma", "common other", "common");
        let tb = toolbox(&global, &project);

        let out = run(&tb, &json!({"query": "common kubernetes"})).unwrap();
        let first_hit = out.lines().nth(1).unwrap();
        assert!(first_hit.starts_with("- beta"), "{out}");
    }

    #[test]
    fn empty_query_is_an_error() {
        let (_dir, global, project) = fixture();
        let tb = toolbox(&global, &project);
        assert!(run(&tb, &json!({"query": ""})).is_err());
        assert!(run(&tb, &json!({"query": "   "})).is_err());
        assert!(run(&tb, &json!({})).is_err());
    }

    #[test]
    fn both_roots_are_scanned_and_project_overrides_global_on_collision() {
        let (_dir, global, project) = fixture();
        write_skill(&global, "shared", "global copy", "zebra shared");
        write_skill(&global, "from-global", "zebra global", "body");
        write_skill(
            &project.join(".agents/skills"),
            "shared",
            "project copy",
            "zebra shared",
        );
        write_skill(
            &project.join(".agents/skills"),
            "from-project",
            "zebra project",
            "body",
        );
        let tb = toolbox(&global, &project);

        let zebra = run(&tb, &json!({"query": "zebra", "limit": 20})).unwrap();
        assert!(zebra.contains("from-global"), "{zebra}");
        assert!(zebra.contains("from-project"), "{zebra}");

        let shared = run(&tb, &json!({"query": "shared"})).unwrap();
        assert!(shared.contains("project copy"), "{shared}");
        assert!(!shared.contains("global copy"), "{shared}");
        let project_path = project.join(".agents/skills/shared/SKILL.md");
        assert!(
            shared.contains(&project_path.display().to_string()),
            "{shared}"
        );
    }

    #[test]
    fn limit_caps_the_hits() {
        let (_dir, global, project) = fixture();
        write_skill(&global, "one", "zebra", "body");
        write_skill(&global, "two", "zebra", "body");
        let tb = toolbox(&global, &project);
        let out = run(&tb, &json!({"query": "zebra", "limit": 1})).unwrap();
        assert!(out.contains("1 match(es)"), "{out}");
    }
}
