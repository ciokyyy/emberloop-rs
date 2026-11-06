use std::io;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct DataRoot {
    root: PathBuf,
}

impl DataRoot {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn projects_file(&self) -> PathBuf {
        self.root.join("projects.json")
    }

    pub fn sessions_dir(&self) -> PathBuf {
        self.root.join("sessions")
    }

    pub fn session_dir(&self, session_id: &str) -> PathBuf {
        self.sessions_dir().join(session_id)
    }

    pub fn session_meta_file(&self, session_id: &str) -> PathBuf {
        self.session_dir(session_id).join("meta.json")
    }

    pub fn session_events_dir(&self, session_id: &str) -> PathBuf {
        self.session_dir(session_id).join("events")
    }

    pub fn session_archive_file(&self, session_id: &str) -> PathBuf {
        self.session_dir(session_id).join("events.zstd")
    }

    pub fn model_catalogue_file(&self) -> PathBuf {
        self.root.join("models").join("catalogue.json")
    }

    pub fn blobs_dir(&self) -> PathBuf {
        self.root.join("blobs")
    }
    pub fn session_events_file(&self, session_id: &str) -> PathBuf {
        self.session_dir(session_id).join("events.jsonl")
    }

    pub fn session_images_dir(&self, session_id: &str) -> PathBuf {
        self.session_dir(session_id).join("images")
    }

    pub fn session_spill_dir(&self, session_id: &str) -> PathBuf {
        self.session_dir(session_id).join("spill")
    }
    pub fn session_splices_file(&self, session_id: &str) -> PathBuf {
        self.session_dir(session_id).join("splices.jsonl")
    }

    pub async fn ensure_layout(&self) -> io::Result<()> {
        tokio::fs::create_dir_all(self.sessions_dir()).await?;
        Ok(())
    }

    pub async fn ensure_session_layout(&self, session_id: &str) -> io::Result<()> {
        let dir = self.session_dir(session_id);
        tokio::fs::create_dir_all(&dir).await?;
        tokio::fs::create_dir_all(self.session_images_dir(session_id)).await?;
        tokio::fs::create_dir_all(self.session_spill_dir(session_id)).await?;
        tokio::fs::create_dir_all(self.session_events_dir(session_id)).await?;
        Ok(())
    }
}
