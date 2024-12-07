use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs;
use tokio::io::{AsyncWriteExt, AsyncReadExt};
use sha2::{Sha256, Digest};
use crate::Result;
use crate::error::CacheError;

const CHUNK_SIZE: usize = 1024 * 1024; // 1MB chunks

pub struct Storage {
    base_path: PathBuf,
}

impl Storage {
    pub fn new<P: AsRef<Path>>(base_path: P) -> Result<Self> {
        let base_path = base_path.as_ref().to_path_buf();
        std::fs::create_dir_all(&base_path)
            .map_err(|e| CacheError::Io(e))?;
        Ok(Self { base_path })
    }

    pub async fn store_chunk(&self, key: &str, chunk_index: u32, data: &[u8]) -> Result<()> {
        let chunk_path = self.get_chunk_path(key, chunk_index);
        fs::write(&chunk_path, data).await
            .map_err(|e| CacheError::Io(e))?;
        Ok(())
    }

    pub async fn read_chunk(&self, key: &str, chunk_index: u32) -> Result<Option<Vec<u8>>> {
        let chunk_path = self.get_chunk_path(key, chunk_index);
        if !chunk_path.exists() {
            return Ok(None);
        }
        
        let data = fs::read(&chunk_path).await
            .map_err(|e| CacheError::Io(e))?;
        Ok(Some(data))
    }

    fn get_chunk_path(&self, key: &str, chunk_index: u32) -> PathBuf {
        let mut hasher = Sha256::new();
        hasher.update(key.as_bytes());
        let hash = format!("{:x}", hasher.finalize());
        
        self.base_path
            .join(&hash[0..2])
            .join(&hash[2..4])
            .join(format!("{}_{}", hash, chunk_index))
    }

    pub async fn merge_chunks(&self, key: &str, total_chunks: u32) -> Result<Vec<u8>> {
        let mut merged = Vec::new();
        for i in 0..total_chunks {
            if let Some(chunk) = self.read_chunk(key, i).await? {
                merged.extend_from_slice(&chunk);
            }
        }
        Ok(merged)
    }

    pub async fn remove(&self, key: &str) -> Result<()> {
        let base_path = self.get_chunk_path(key, 0).parent().unwrap().to_path_buf();
        if base_path.exists() {
            tokio::fs::remove_dir_all(base_path).await
                .map_err(|e| CacheError::Io(e))?;
        }
        Ok(())
    }

    pub async fn get_size(&self, key: &str) -> Result<u64> {
        let mut total_size = 0;
        let base_path = self.get_chunk_path(key, 0).parent().unwrap().to_path_buf();
        
        if base_path.exists() {
            let mut read_dir = tokio::fs::read_dir(base_path).await
                .map_err(|e| CacheError::Io(e))?;
                
            while let Ok(Some(entry)) = read_dir.next_entry().await {
                if let Ok(metadata) = entry.metadata().await {
                    total_size += metadata.len();
                }
            }
        }
        
        Ok(total_size)
    }
} 