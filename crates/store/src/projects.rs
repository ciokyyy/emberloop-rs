use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Project {
    pub id: String,
    pub name: String,
    #[serde(alias = "path")]
    pub root: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum ProjectStoreError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Serialization error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("Project not found: {0}")]
    NotFound(String),
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
    ) -> Result<Project, ProjectStoreError> {
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
        };

        store.insert(p1.clone()).await.unwrap();
        assert_eq!(store.get("proj_1").await, Some(p1.clone()));

        store
            .update("proj_1", Some("Updated Name".into()), None)
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
}
