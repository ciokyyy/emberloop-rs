use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Project {
    pub id: String,
    pub name: String,
    #[serde(alias = "path")]
    pub root: PathBuf,
    #[serde(default)]
    pub tools: Vec<ProjectTool>,
}

/// One MCP server a Project's Sessions may use, matched to `mcp.json` by `name`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProjectTool {
    pub name: String,
    #[serde(default)]
    pub url: String,
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
}

fn enabled_by_default() -> bool {
    true
}

#[derive(Debug, thiserror::Error)]
pub enum ProjectStoreError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Serialization error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("Project not found: {0}")]
    NotFound(String),
    #[error("{0}")]
    Invalid(String),
}

#[derive(Clone)]
pub struct ProjectsStore {
    path: PathBuf,
    projects: Arc<RwLock<HashMap<String, Project>>>,
}

impl ProjectsStore {
    pub async fn load(path: impl Into<PathBuf>) -> Result<Self, ProjectStoreError> {
        let path = path.into();
        let mut map = HashMap::new();
        if path.exists() {
            let content = tokio::fs::read_to_string(&path).await?;
            if !content.trim().is_empty() {
                let list: Vec<Project> = serde_json::from_str(&content)?;
                for p in list {
                    map.insert(p.id.clone(), p);
                }
            }
        }

        Ok(Self {
            path,
            projects: Arc::new(RwLock::new(map)),
        })
    }

    async fn persist(&self, projects: &HashMap<String, Project>) -> Result<(), ProjectStoreError> {
        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let list: Vec<Project> = projects.values().cloned().collect();
        let json = serde_json::to_string_pretty(&list)?;
        let tmp_path = format!("{}.tmp.{}", self.path.display(), std::process::id());
        tokio::fs::write(&tmp_path, json).await?;
        tokio::fs::rename(&tmp_path, &self.path).await?;
        Ok(())
    }

    pub async fn list(&self) -> Vec<Project> {
        let map = self.projects.read().await;
        map.values().cloned().collect()
    }

    pub async fn get(&self, id: &str) -> Option<Project> {
        let map = self.projects.read().await;
        map.get(id).cloned()
    }

    pub async fn insert(&self, project: Project) -> Result<Project, ProjectStoreError> {
        validate_tools(&project.tools)?;
        let mut map = self.projects.write().await;
        map.insert(project.id.clone(), project.clone());
        self.persist(&map).await?;
        Ok(project)
    }

    pub async fn update(
        &self,
        id: &str,
        name: Option<String>,
        root: Option<PathBuf>,
        tools: Option<Vec<ProjectTool>>,
    ) -> Result<Project, ProjectStoreError> {
        if let Some(tools) = &tools {
            validate_tools(tools)?;
        }
        let mut map = self.projects.write().await;
        let p = map
            .get_mut(id)
            .ok_or_else(|| ProjectStoreError::NotFound(id.to_string()))?;
        if let Some(name) = name {
            p.name = name;
        }
        if let Some(root) = root {
            p.root = root;
        }
        if let Some(tools) = tools {
            p.tools = tools;
        }
        let updated = p.clone();
        self.persist(&map).await?;
        Ok(updated)
    }

    pub async fn delete(&self, id: &str) -> Result<Project, ProjectStoreError> {
        let mut map = self.projects.write().await;
        let removed = map
            .remove(id)
            .ok_or_else(|| ProjectStoreError::NotFound(id.to_string()))?;
        self.persist(&map).await?;
        Ok(removed)
    }
}

fn validate_tools(tools: &[ProjectTool]) -> Result<(), ProjectStoreError> {
    if tools.len() > 32 {
        return Err(ProjectStoreError::Invalid(
            "project tools: at most 32 tools".into(),
        ));
    }
    let mut seen = HashSet::new();
    for tool in tools {
        if !valid_tool_name(&tool.name) {
            return Err(ProjectStoreError::Invalid(format!(
                "project tools: name {:?} is invalid",
                tool.name
            )));
        }
        if !seen.insert(tool.name.as_str()) {
            return Err(ProjectStoreError::Invalid(format!(
                "project tools: duplicate name {:?}",
                tool.name
            )));
        }
        if !tool.url.is_empty()
            && !(tool.url.starts_with("http://") || tool.url.starts_with("https://"))
        {
            return Err(ProjectStoreError::Invalid(
                "project tools: url must be http(s)".into(),
            ));
        }
    }
    Ok(())
}

fn valid_tool_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    let Some((head, body)) = bytes.split_first() else {
        return false;
    };
    if bytes.len() > 64 || !(head.is_ascii_lowercase() || head.is_ascii_digit()) {
        return false;
    }
    body.iter()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_projects_crud() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("projects.json");
        let store = ProjectsStore::load(&file).await.unwrap();

        let p1 = Project {
            id: "proj_1".into(),
            name: "Test Project".into(),
            root: PathBuf::from("/tmp/test"),
            tools: Vec::new(),
        };

        store.insert(p1.clone()).await.unwrap();
        assert_eq!(store.get("proj_1").await, Some(p1.clone()));

        store
            .update("proj_1", Some("Updated Name".into()), None, None)
            .await
            .unwrap();
        let updated = store.get("proj_1").await.unwrap();
        assert_eq!(updated.name, "Updated Name");

        // Reload
        let store2 = ProjectsStore::load(&file).await.unwrap();
        assert_eq!(store2.get("proj_1").await.unwrap().name, "Updated Name");

        store2.delete("proj_1").await.unwrap();
        assert!(store2.get("proj_1").await.is_none());
    }

    fn tool(name: &str, url: &str, enabled: bool) -> ProjectTool {
        ProjectTool {
            name: name.into(),
            url: url.into(),
            enabled,
        }
    }

    async fn reject(store: &ProjectsStore, tools: Vec<ProjectTool>) -> String {
        match store.update("p", None, None, Some(tools)).await {
            Err(ProjectStoreError::Invalid(message)) => message,
            other => panic!("expected invalid tools, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn tools_round_trip_and_an_old_file_loads_with_defaults() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("projects.json");
        std::fs::write(
            &file,
            r#"[{"id":"old","name":"Old","root":"/tmp/old"},{"id":"legacy","name":"Legacy","root":"/tmp/legacy","tools":[{"name":"solo"}]}]"#,
        )
        .unwrap();
        let store = ProjectsStore::load(&file).await.unwrap();
        assert!(store.get("old").await.unwrap().tools.is_empty());
        assert_eq!(
            store.get("legacy").await.unwrap().tools,
            vec![tool("solo", "", true)]
        );

        let flavors = vec![
            tool("photocraft", "https://box.ts.net/mcp", true),
            tool("docs", "", false),
        ];
        store
            .insert(Project {
                id: "p".into(),
                name: "P".into(),
                root: PathBuf::from("/tmp/p"),
                tools: flavors.clone(),
            })
            .await
            .unwrap();
        store
            .update("p", None, None, Some(vec![flavors[1].clone()]))
            .await
            .unwrap();
        let reloaded = ProjectsStore::load(&file).await.unwrap();
        assert_eq!(
            reloaded.get("p").await.unwrap().tools,
            vec![tool("docs", "", false)]
        );
        assert!(reloaded.get("old").await.unwrap().tools.is_empty());
        assert_eq!(
            reloaded.get("legacy").await.unwrap().tools,
            vec![tool("solo", "", true)]
        );

        store.update("p", None, None, Some(vec![])).await.unwrap();
        assert!(store.get("p").await.unwrap().tools.is_empty());
    }

    #[tokio::test]
    async fn tools_are_validated() {
        let dir = tempdir().unwrap();
        let store = ProjectsStore::load(dir.path().join("projects.json"))
            .await
            .unwrap();
        store
            .insert(Project {
                id: "p".into(),
                name: "P".into(),
                root: PathBuf::from("/tmp/p"),
                tools: Vec::new(),
            })
            .await
            .unwrap();

        assert_eq!(
            reject(&store, vec![tool("PhotoCraft", "", true)]).await,
            "project tools: name \"PhotoCraft\" is invalid"
        );
        assert_eq!(
            reject(
                &store,
                vec![tool("photocraft", "", true), tool("photocraft", "", false)]
            )
            .await,
            "project tools: duplicate name \"photocraft\""
        );
        let many = (0..33)
            .map(|n| tool(&format!("tool-{n}"), "", true))
            .collect();
        assert_eq!(
            reject(&store, many).await,
            "project tools: at most 32 tools"
        );
        assert_eq!(
            reject(&store, vec![tool("photos", "ftp://box/mcp", true)]).await,
            "project tools: url must be http(s)"
        );
    }
}
