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

/// Ranked skills for `query`, each with its module, origin, clipped description and SKILL.md
/// path. A query naming a module lists every member first, heading by the module's `about`.
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
    let found: Vec<skills::Skill> = skills::discover(toolbox.global_skills(), toolbox.root())
        .into_iter()
        .filter(|skill| !skill.hidden)
        .collect();
    let modules = skills::modules(toolbox.global_skills(), toolbox.root());
    let docs: Vec<Vec<String>> = found
        .iter()
        .map(|skill| bm25::tokens(&search_text(skill, &modules)))
        .collect();
    let mut hits: Vec<(f64, &skills::Skill)> = bm25::scores(&docs, &bm25::query_terms(query))
        .into_iter()
        .zip(&found)
        .filter(|(score, _)| *score > 0.0)
        .collect();
    hits.sort_by(|a, b| b.0.total_cmp(&a.0));
    let named = modules
        .iter()
        .find(|module| module.name.eq_ignore_ascii_case(query));
    let members: Vec<&skills::Skill> = named
        .map(|module| {
            module
                .skills
                .iter()
                .filter_map(|name| found.iter().find(|skill| &skill.name == name))
                .collect()
        })
        .unwrap_or_default();
    let mut ordered: Vec<&skills::Skill> = members.clone();
    ordered.extend(
        hits.iter()
            .map(|(_, skill)| *skill)
            .filter(|skill| !members.iter().any(|member| member.name == skill.name)),
    );
    ordered.truncate(members.len() + limit);
    if ordered.is_empty() {
        return Ok(format!("query {query:?}: nothing matched\n"));
    }
    let mut out = format!("query {query:?}: {} match(es)\n", ordered.len());
    if let Some(module) = named {
        out.push_str(&format!(
            "module {}: {}\n",
            module.name,
            clipped(&module.about)
        ));
    }
    for skill in ordered {
        let module = skills::module_of(&modules, &skill.name)
            .map(|module| format!(" [{module}]"))
            .unwrap_or_default();
        out.push_str(&format!(
            "- {}{module} [{}]: {description}\n  {}\n",
            skill.name,
            skill.origin,
            skill.path.display(),
            description = clipped(&skill.description)
        ));
    }
    Ok(out)
}

/// What `skill_search` ranks a skill by: its module name, then its own search text.
fn search_text(skill: &skills::Skill, modules: &[skills::Module]) -> String {
    match skills::module_of(modules, &skill.name) {
        Some(module) => format!("{module} {}", skill.search_text()),
        None => skill.search_text(),
    }
}

fn clipped(text: &str) -> String {
    text.chars().take(DESCRIPTION_CHARS).collect()
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

    fn write_modules(root: &Path, text: &str) {
        fs::write(root.join(crate::skills::MODULES_FILE), text).unwrap();
    }

    fn line_index(out: &str, start: &str) -> usize {
        out.lines()
            .position(|line| line.starts_with(start))
            .unwrap_or_else(|| panic!("no line starting {start:?} in {out}"))
    }

    #[test]
    fn module_name_query_lists_all_members_first_and_shows_about() {
        let (_dir, global, project) = fixture();
        write_skill(&global, "clip", "trim video", "ffmpeg");
        write_skill(&global, "sound", "mix audio", "sox");
        write_skill(
            &global,
            "loud",
            "media media media",
            &"media media media\n".repeat(20),
        );
        write_modules(
            &global,
            r#"{"media": {"about": "audio and video work", "skills": ["clip", "sound"]}}"#,
        );
        let tb = toolbox(&global, &project);

        let out = run(&tb, &json!({"query": "Media", "limit": 1})).unwrap();
        assert!(out.contains("audio and video work"), "{out}");
        assert!(out.contains("trim video"), "{out}");
        assert!(out.contains("mix audio"), "{out}");
        let clip = line_index(&out, "- clip [media] [human]");
        let sound = line_index(&out, "- sound [media] [human]");
        assert!(clip < sound, "{out}");
        assert!(sound < line_index(&out, "- loud "), "{out}");
    }

    #[test]
    fn module_name_is_part_of_the_search_text() {
        let (_dir, global, project) = fixture();
        write_skill(&global, "clip", "trim video", "ffmpeg");
        write_skill(&global, "sound", "mix audio", "sox");
        write_skill(&global, "noise", "zebra", "body");
        write_modules(
            &global,
            r#"{"media-kit": {"about": "audio and video work", "skills": ["clip", "sound"]}}"#,
        );
        let tb = toolbox(&global, &project);

        let out = run(&tb, &json!({"query": "media"})).unwrap();
        assert!(out.contains("clip") && out.contains("sound"), "{out}");
        assert!(!out.contains("noise"), "{out}");
    }

    #[test]
    fn module_member_hit_shows_the_module_tag() {
        let (_dir, global, project) = fixture();
        write_skill(&global, "clip", "trim video", "ffmpeg");
        write_modules(
            &global,
            r#"{"media": {"about": "audio and video work", "skills": ["clip"]}}"#,
        );
        let tb = toolbox(&global, &project);

        let out = run(&tb, &json!({"query": "clip"})).unwrap();
        assert!(out.contains("- clip [media] [human]: trim video"), "{out}");
    }

    #[test]
    fn unmapped_hit_line_is_unchanged() {
        let (_dir, global, project) = fixture();
        write_skill(&global, "plain", "zebra helper", "body");
        let tb = toolbox(&global, &project);

        let out = run(&tb, &json!({"query": "zebra"})).unwrap();
        assert!(out.contains("- plain [human]: zebra helper"), "{out}");
    }

    #[test]
    fn hidden_skills_are_not_searched() {
        let (_dir, global, project) = fixture();
        write_skill(&global, "visible", "deploy helper", "rollout");
        let folder = global.join("quiet");
        fs::create_dir_all(&folder).unwrap();
        fs::write(
            folder.join("SKILL.md"),
            "---\nname: quiet\ndescription: deploy helper\ndisable-model-invocation: true\n---\nrollout\n",
        )
        .unwrap();
        let tb = toolbox(&global, &project);

        let out = run(&tb, &json!({"query": "deploy"})).unwrap();
        assert!(out.contains("- visible"), "{out}");
        assert!(!out.contains("quiet"), "{out}");
        assert_eq!(out.lines().next(), Some("query \"deploy\": 1 match(es)"));
    }
}
