use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::DataRoot;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    #[default]
    Active,
    Archived,
    Restored,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionMeta {
    pub id: String,
    #[serde(rename = "projectId")]
    pub project_id: String,
    pub title: String,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(rename = "thoughtLevel", default)]
    pub thought_level: Option<String>,
    #[serde(rename = "createdAt")]
    pub created_at: u64,
    #[serde(rename = "updatedAt")]
    pub updated_at: u64,
    /// Content hash of every prompt the Session pinned, by prompt id.
    #[serde(rename = "promptHashes", default)]
    pub prompt_hashes: HashMap<String, String>,
    #[serde(rename = "lastUsedAt", default)]
    pub last_used_at: Option<u64>,
    #[serde(rename = "archivedAt", default)]
    pub archived_at: Option<u64>,
    #[serde(rename = "restoredAt", default)]
    pub restored_at: Option<u64>,
    #[serde(rename = "restoredSeq", default)]
    pub restored_seq: Option<u64>,
    #[serde(default)]
    pub state: SessionState,
    /// The harness's own id for this Session's conversation, so a restarted
    /// daemon can load it again instead of starting an empty one.
    #[serde(rename = "resumeToken", default)]
    pub resume_token: Option<String>,
}

impl Default for SessionMeta {
    fn default() -> Self {
        Self {
            id: String::new(),
            project_id: String::new(),
            title: String::new(),
            model: default_model(),
            thought_level: None,
            created_at: 0,
            updated_at: 0,
            prompt_hashes: HashMap::new(),
            last_used_at: None,
            archived_at: None,
            restored_at: None,
            restored_seq: None,
            state: SessionState::Active,
            resume_token: None,
        }
    }
}

fn default_model() -> String {
    "gemini-3.8-flash".to_string()
}

#[derive(Debug, thiserror::Error)]
pub enum SessionStoreError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Serialization error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("Session not found: {0}")]
    NotFound(String),
}

#[derive(Clone)]
pub struct SessionsStore {
    data_root: DataRoot,
    sessions: Arc<RwLock<HashMap<String, SessionMeta>>>,
}

impl SessionsStore {
    pub async fn load(data_root: DataRoot) -> Result<Self, SessionStoreError> {
        data_root.ensure_layout().await?;
        let mut map = HashMap::new();
        let sessions_dir = data_root.sessions_dir();
        if sessions_dir.exists() {
            let mut read_dir = tokio::fs::read_dir(&sessions_dir).await?;
            while let Some(entry) = read_dir.next_entry().await? {
                if entry.file_type().await?.is_dir() {
                    let session_id = entry.file_name().to_string_lossy().to_string();
                    let meta_file = data_root.session_meta_file(&session_id);
                    if meta_file.exists() {
                        let content = tokio::fs::read_to_string(&meta_file).await?;
                        if let Ok(meta) = serde_json::from_str::<SessionMeta>(&content) {
                            map.insert(meta.id.clone(), meta);
                        }
                    }
                }
            }
        }

        Ok(Self {
            data_root,
            sessions: Arc::new(RwLock::new(map)),
        })
    }

    pub async fn list(&self, project_id: Option<&str>) -> Vec<SessionMeta> {
        let map = self.sessions.read().await;
        map.values()
            .filter(|s| project_id.is_none() || Some(s.project_id.as_str()) == project_id)
            .cloned()
            .collect()
    }

    pub async fn get(&self, id: &str) -> Option<SessionMeta> {
        let map = self.sessions.read().await;
        map.get(id).cloned()
    }

    pub async fn insert(&self, meta: SessionMeta) -> Result<SessionMeta, SessionStoreError> {
        self.data_root.ensure_session_layout(&meta.id).await?;
        self.save(&meta).await?;
        Ok(meta)
    }

    /// Writes metadata atomically and refreshes the in-memory copy. It does not
    /// recreate the event directory, so an archived Session stays archived.
    pub async fn save(&self, meta: &SessionMeta) -> Result<(), SessionStoreError> {
        let mut map = self.sessions.write().await;
        let mut meta = meta.clone();
        // Stale harness or archive metadata must not undo a generated or manual title.
        if meta.title.is_empty() || meta.title == "New Session" {
            if let Some(current) = map.get(&meta.id) {
                meta.title.clone_from(&current.title);
            }
        }
        let meta_file = self.data_root.session_meta_file(&meta.id);
        let json = serde_json::to_string_pretty(&meta)?;
        let tmp_path = format!("{}.tmp.{}", meta_file.display(), std::process::id());
        tokio::fs::write(&tmp_path, json).await?;
        tokio::fs::rename(&tmp_path, &meta_file).await?;
        map.insert(meta.id.clone(), meta);
        Ok(())
    }

    pub async fn touch(&self, id: &str, now: u64) -> Result<SessionMeta, SessionStoreError> {
        let mut meta = self
            .get(id)
            .await
            .ok_or_else(|| SessionStoreError::NotFound(id.to_string()))?;
        meta.last_used_at = Some(now);
        meta.updated_at = now;
        self.save(&meta).await?;
        Ok(meta)
    }

    pub async fn update_title(
        &self,
        id: &str,
        title: &str,
    ) -> Result<SessionMeta, SessionStoreError> {
        let mut map = self.sessions.write().await;
        let s = map
            .get_mut(id)
            .ok_or_else(|| SessionStoreError::NotFound(id.to_string()))?;
        s.title = title.to_string();
        s.updated_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let updated = s.clone();

        let meta_file = self.data_root.session_meta_file(id);
        let json = serde_json::to_string_pretty(&updated)?;
        let tmp_path = format!("{}.tmp.{}", meta_file.display(), std::process::id());
        tokio::fs::write(&tmp_path, json).await?;
        tokio::fs::rename(&tmp_path, &meta_file).await?;

        Ok(updated)
    }

    /// Commit a generated title only while the Session still has its default title.
    pub async fn set_generated_title(
        &self,
        id: &str,
        title: &str,
    ) -> Result<Option<SessionMeta>, SessionStoreError> {
        let mut map = self.sessions.write().await;
        let Some(meta) = map
            .get(id)
            .filter(|meta| meta.title.is_empty() || meta.title == "New Session")
        else {
            return Ok(None);
        };
        let mut updated = meta.clone();
        updated.title = title.to_string();
        let path = self.data_root.session_meta_file(id);
        let tmp = path.with_extension("title.tmp");
        tokio::fs::write(&tmp, serde_json::to_string_pretty(&updated)?).await?;
        tokio::fs::rename(tmp, path).await?;
        map.insert(id.to_string(), updated.clone());
        Ok(Some(updated))
    }

    pub async fn update_thought_level(
        &self,
        id: &str,
        thought_level: Option<String>,
    ) -> Result<SessionMeta, SessionStoreError> {
        let mut meta = self
            .get(id)
            .await
            .ok_or_else(|| SessionStoreError::NotFound(id.to_string()))?;
        meta.thought_level = thought_level.filter(|l| !l.trim().is_empty());
        meta.updated_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        self.save(&meta).await?;
        Ok(meta)
    }

    pub async fn set_prompt_hashes(
        &self,
        id: &str,
        hashes: HashMap<String, String>,
    ) -> Result<SessionMeta, SessionStoreError> {
        let mut map = self.sessions.write().await;
        let session = map
            .get_mut(id)
            .ok_or_else(|| SessionStoreError::NotFound(id.to_string()))?;
        session.prompt_hashes = hashes;
        session.updated_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let updated = session.clone();

        let meta_file = self.data_root.session_meta_file(id);
        let json = serde_json::to_string_pretty(&updated)?;
        let tmp_path = format!("{}.tmp.{}", meta_file.display(), std::process::id());
        tokio::fs::write(&tmp_path, json).await?;
        tokio::fs::rename(&tmp_path, &meta_file).await?;

        Ok(updated)
    }

    pub async fn delete(&self, id: &str) -> Result<SessionMeta, SessionStoreError> {
        let mut map = self.sessions.write().await;
        let removed = map
            .remove(id)
            .ok_or_else(|| SessionStoreError::NotFound(id.to_string()))?;
        let dir = self.data_root.session_dir(id);
        if dir.exists() {
            tokio::fs::remove_dir_all(&dir).await?;
        }
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_sessions_crud() {
        let dir = tempdir().unwrap();
        let data_root = DataRoot::new(dir.path());
        let store = SessionsStore::load(data_root.clone()).await.unwrap();

        let meta = SessionMeta {
            id: "ses_1".into(),
            project_id: "proj_1".into(),
            title: "First Session".into(),
            created_at: 1000,
            updated_at: 1000,
            prompt_hashes: HashMap::new(),
            ..SessionMeta::default()
        };

        store.insert(meta.clone()).await.unwrap();
        assert_eq!(store.get("ses_1").await, Some(meta.clone()));

        store.update_title("ses_1", "New Title").await.unwrap();
        let updated = store.get("ses_1").await.unwrap();
        assert_eq!(updated.title, "New Title");

        let hashes = HashMap::from([("policy".to_string(), "abc123".to_string())]);
        store
            .set_prompt_hashes("ses_1", hashes.clone())
            .await
            .unwrap();
        assert_eq!(store.get("ses_1").await.unwrap().prompt_hashes, hashes);

        let store2 = SessionsStore::load(data_root.clone()).await.unwrap();
        assert_eq!(store2.get("ses_1").await.unwrap().title, "New Title");

        store2.delete("ses_1").await.unwrap();
        assert!(store2.get("ses_1").await.is_none());
        assert!(!data_root.session_dir("ses_1").exists());
    }
}

#[cfg(test)]
mod generated_title_tests {
    use super::*;

    #[tokio::test]
    async fn generated_title_never_overwrites_a_manual_title() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionsStore::load(DataRoot::new(dir.path()))
            .await
            .unwrap();
        let meta = SessionMeta {
            id: "test".into(),
            title: "New Session".into(),
            ..Default::default()
        };
        store.insert(meta.clone()).await.unwrap();
        store.update_title("test", "Mine").await.unwrap();
        assert!(store
            .set_generated_title("test", "Generated")
            .await
            .unwrap()
            .is_none());
        assert_eq!(store.get("test").await.unwrap().title, "Mine");
        store.update_title("test", "New Session").await.unwrap();
        assert!(store
            .set_generated_title("test", "Generated")
            .await
            .unwrap()
            .is_some());
        assert_eq!(store.get("test").await.unwrap().title, "Generated");
        store.save(&meta).await.unwrap();
        assert_eq!(store.get("test").await.unwrap().title, "Generated");
        let reloaded = SessionsStore::load(DataRoot::new(dir.path()))
            .await
            .unwrap();
        assert_eq!(reloaded.get("test").await.unwrap().title, "Generated");
        assert!(store
            .set_generated_title("missing", "No")
            .await
            .unwrap()
            .is_none());
    }
}
