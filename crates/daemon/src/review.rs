//! Runs the proxy's background skill review tool calls against a Session's Project. The proxy
//! has no workspace tools; this executor owns the whitelist.

use std::path::PathBuf;

use async_trait::async_trait;
use proxy::review::ReviewTools;
use serde_json::Value;
use store::{DataRoot, ProjectsStore, SessionsStore};
use tools::{ToolConfig, Toolbox};

/// The review may write skills, stage prompt proposals for the User, and look around; it may not
/// run commands, edit files, or change a prompt itself.
const ALLOWED: &[&str] = &[
    "skill_manage",
    "skill_search",
    "prompt_propose",
    "read",
    "grep",
    "find",
];

pub(crate) struct ReviewExecutor {
    pub sessions: SessionsStore,
    pub projects: ProjectsStore,
    pub data_root: DataRoot,
    pub global_skills: PathBuf,
    pub config_root: PathBuf,
}

impl std::fmt::Debug for ReviewExecutor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReviewExecutor").finish_non_exhaustive()
    }
}

#[async_trait]
impl ReviewTools for ReviewExecutor {
    async fn call(&self, session_id: &str, name: &str, args: &Value) -> Result<String, String> {
        let short = crate::tooldisplay::short_tool_name(name);
        if !ALLOWED.contains(&short) {
            return Err(format!(
                "Background review denied non-whitelisted tool: {name}. Only skill tools, prompt_propose and read-only file tools are allowed."
            ));
        }
        let meta = self
            .sessions
            .get(session_id)
            .await
            .ok_or("the Session is gone")?;
        let project = self
            .projects
            .get(&meta.project_id)
            .await
            .ok_or("the Project is gone")?;
        let toolbox = Toolbox::new(ToolConfig {
            global_skills: Some(self.global_skills.clone()),
            prompts_dir: Some(self.config_root.join("prompts")),
            ..ToolConfig::new(project.root, self.data_root.session_spill_dir(session_id))
        })
        .map_err(|error| error.to_string())?;
        toolbox
            .call(short, args)
            .await
            .map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn a_tool_outside_the_whitelist_is_refused_before_any_lookup() {
        let dir = tempfile::tempdir().unwrap();
        let data_root = DataRoot::new(dir.path());
        let executor = ReviewExecutor {
            sessions: SessionsStore::load(data_root.clone()).await.unwrap(),
            projects: ProjectsStore::load(data_root.projects_file())
                .await
                .unwrap(),
            data_root,
            global_skills: dir.path().join("skills"),
            config_root: dir.path().join("config"),
        };
        for name in ["bash", "mcp__el__edit", "write"] {
            let error = executor.call("ses_1", name, &json!({})).await.unwrap_err();
            assert_eq!(
                error,
                format!(
                    "Background review denied non-whitelisted tool: {name}. Only skill tools, prompt_propose and read-only file tools are allowed."
                )
            );
        }
        // Allowed: it gets past the whitelist and stops at the missing Session.
        let error = executor
            .call("ses_1", "mcp__el__prompt_propose", &json!({}))
            .await
            .unwrap_err();
        assert_eq!(error, "the Session is gone");
    }
}
