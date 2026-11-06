use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum BlobError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("blob not found: {0}")]
    NotFound(String),
    #[error("invalid blob hash: {0}")]
    InvalidHash(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlobRef {
    pub hash: String,
    pub size: u64,
}

/// Content-addressed byte store: one file per sha256, never a duplicate.
#[derive(Clone, Debug)]
pub struct BlobStore {
    dir: PathBuf,
}

impl BlobStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub async fn put(&self, bytes: &[u8]) -> Result<BlobRef, BlobError> {
        tokio::fs::create_dir_all(&self.dir).await?;
        let hash = hex::encode(Sha256::digest(bytes));
        let path = self.path(&hash);
        if !path.exists() {
            let tmp = self.dir.join(format!("{hash}.tmp.{}", std::process::id()));
            tokio::fs::write(&tmp, bytes).await?;
            tokio::fs::rename(&tmp, &path).await?;
        }
        Ok(BlobRef {
            hash,
            size: bytes.len() as u64,
        })
    }

    pub async fn get(&self, hash: &str) -> Result<Vec<u8>, BlobError> {
        if !valid_hash(hash) {
            return Err(BlobError::InvalidHash(hash.to_string()));
        }
        let path = self.path(hash);
        tokio::fs::read(&path).await.map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                BlobError::NotFound(hash.to_string())
            } else {
                BlobError::Io(error)
            }
        })
    }

    pub fn path(&self, hash: &str) -> PathBuf {
        self.dir.join(hash)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

fn valid_hash(hash: &str) -> bool {
    hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
}
