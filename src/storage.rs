use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use crate::error::PluginError;
use tracing::{info, warn, error, debug};

#[derive(Debug)]
pub struct StorageManager {
    root_path: PathBuf,
    state: Arc<RwLock<StorageState>>,
    config: StorageConfig,
}

#[derive(Debug)]
struct StorageState {
    files: HashMap<String, FileInfo>,
    used_space: u64,
    max_space: u64,
    last_cleanup: Instant,
}

#[derive(Debug)]
struct FileInfo {
    path: PathBuf,
    size: u64,
    last_access: Instant,
    metadata: FileMetadata,
}

#[derive(Debug, Clone)]
pub struct FileMetadata {
    pub content_type: String,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug)]
pub struct StorageConfig {
    pub max_space: u64,
    pub cleanup_interval: Duration,
    pub file_ttl: Duration,
    pub min_free_space: u64,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            max_space: 1024 * 1024 * 1024, // 1GB
            cleanup_interval: Duration::from_secs(3600), // 1 hour
            file_ttl: Duration::from_secs(86400), // 24 hours
            min_free_space: 1024 * 1024 * 100, // 100MB
        }
    }
}

impl StorageManager {
    pub fn new<P: AsRef<Path>>(root_path: P, config: StorageConfig) -> Self {
        info!("Initializing storage manager with root path: {:?}", root_path.as_ref());
        debug!("Storage config: {:?}", config);

        Self {
            root_path: root_path.as_ref().to_owned(),
            state: Arc::new(RwLock::new(StorageState {
                files: HashMap::new(),
                used_space: 0,
                max_space: config.max_space,
                last_cleanup: Instant::now(),
            })),
            config,
        }
    }

    pub async fn store_file(
        &self,
        key: String,
        data: Vec<u8>,
        metadata: FileMetadata,
    ) -> Result<(), PluginError> {
        debug!("Attempting to store file: {} ({} bytes)", key, data.len());
        let mut state = self.state.write().await;

        // 检查空间
        let file_size = data.len() as u64;
        if state.used_space + file_size > state.max_space {
            warn!(
                "Storage space exceeded. Current: {}, Required: {}, Max: {}",
                state.used_space, file_size, state.max_space
            );

            // 尝试清理空间
            drop(state);
            self.cleanup_expired_files().await?;
            state = self.state.write().await;

            // 再次检查空间
            if state.used_space + file_size > state.max_space {
                return Err(PluginError::Storage("Storage space exceeded".into()));
            }
        }

        // 存储文件
        let file_path = self.root_path.join(&key);
        match tokio::fs::write(&file_path, &data).await {
            Ok(_) => {
                state.files.insert(key.clone(), FileInfo {
                    path: file_path.clone(),
                    size: file_size,
                    last_access: Instant::now(),
                    metadata,
                });
                state.used_space += file_size;
                info!("Successfully stored file: {} ({} bytes)", key, file_size);
                Ok(())
            }
            Err(e) => {
                error!("Failed to store file {}: {}", key, e);
                Err(PluginError::Storage(e.to_string()))
            }
        }
    }

    pub async fn get_file(&self, key: &str) -> Result<(Vec<u8>, FileMetadata), PluginError> {
        debug!("Attempting to retrieve file: {}", key);
        let mut state = self.state.write().await;

        if let Some(file_info) = state.files.get_mut(key) {
            file_info.last_access = Instant::now();
            match tokio::fs::read(&file_info.path).await {
                Ok(data) => {
                    info!("Successfully retrieved file: {} ({} bytes)", key, data.len());
                    Ok((data, file_info.metadata.clone()))
                }
                Err(e) => {
                    error!("Failed to read file {}: {}", key, e);
                    Err(PluginError::Storage(e.to_string()))
                }
            }
        } else {
            warn!("File not found: {}", key);
            Err(PluginError::Storage("File not found".into()))
        }
    }

    pub async fn cleanup_expired_files(&self) -> Result<(), PluginError> {
        info!("Starting expired files cleanup");
        let mut state = self.state.write().await;
        let now = Instant::now();
        let mut removed_count = 0;
        let mut freed_space = 0;

        // 找出过期文件
        let expired: Vec<_> = state.files.iter()
            .filter(|(_, info)| now.duration_since(info.last_access) > self.config.file_ttl)
            .map(|(k, _)| k.clone())
            .collect();

        debug!("Found {} expired files", expired.len());

        // 删除过期文件
        for key in expired {
            if let Some(info) = state.files.remove(&key) {
                match tokio::fs::remove_file(&info.path).await {
                    Ok(_) => {
                        state.used_space -= info.size;
                        freed_space += info.size;
                        removed_count += 1;
                        debug!("Removed expired file: {}", key);
                    }
                    Err(e) => warn!("Failed to remove file {}: {}", key, e),
                }
            }
        }

        state.last_cleanup = now;
        info!(
            "Cleanup completed: removed {} files, freed {} bytes",
            removed_count, freed_space
        );
        Ok(())
    }

    pub async fn get_storage_stats(&self) -> StorageStats {
        let state = self.state.read().await;
        let stats = StorageStats {
            total_space: state.max_space,
            used_space: state.used_space,
            free_space: state.max_space - state.used_space,
            file_count: state.files.len(),
            last_cleanup: state.last_cleanup,
        };
        debug!("Storage stats: {:?}", stats);
        stats
    }

    pub async fn start_cleanup_task(self: Arc<Self>) {
        info!("Starting storage cleanup task");
        let cleanup_interval = self.config.cleanup_interval;

        tokio::spawn(async move {
            loop {
                tokio::time::sleep(cleanup_interval).await;
                debug!("Running scheduled cleanup");
                if let Err(e) = self.cleanup_expired_files().await {
                    error!("Error during cleanup: {}", e);
                }
            }
        });
    }
}

#[derive(Debug)]
pub struct StorageStats {
    pub total_space: u64,
    pub used_space: u64,
    pub free_space: u64,
    pub file_count: usize,
    pub last_cleanup: Instant,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::test;
    use std::time::Duration;

    #[test]
    async fn test_basic_storage() {
        let temp_dir = tempfile::tempdir().unwrap();
        let storage = StorageManager::new(
            temp_dir.path(),
            StorageConfig::default(),
        );

        // Store a file
        let data = b"test data".to_vec();
        let metadata = FileMetadata {
            content_type: "text/plain".to_string(),
            etag: Some("123".to_string()),
            last_modified: None,
            created_at: chrono::Utc::now(),
        };

        storage.store_file("test.txt".to_string(), data.clone(), metadata.clone())
            .await
            .unwrap();

        // Retrieve the file
        let (retrieved_data, retrieved_metadata) = storage.get_file("test.txt")
            .await
            .unwrap();

        assert_eq!(retrieved_data, data);
        assert_eq!(retrieved_metadata.content_type, "text/plain");
        assert_eq!(retrieved_metadata.etag, Some("123".to_string()));
    }

    #[test]
    async fn test_cleanup() {
        let temp_dir = tempfile::tempdir().unwrap();
        let storage = StorageManager::new(
            temp_dir.path(),
            StorageConfig {
                file_ttl: Duration::from_secs(1),
                ..Default::default()
            },
        );

        // Store some files
        let data = b"test data".to_vec();
        let metadata = FileMetadata {
            content_type: "text/plain".to_string(),
            etag: None,
            last_modified: None,
            created_at: chrono::Utc::now(),
        };

        storage.store_file("file1.txt".to_string(), data.clone(), metadata.clone())
            .await
            .unwrap();
        storage.store_file("file2.txt".to_string(), data.clone(), metadata.clone())
            .await
            .unwrap();

        // Wait for files to expire
        tokio::time::sleep(Duration::from_secs(2)).await;

        // Run cleanup
        storage.cleanup_expired_files().await.unwrap();

        // Check that files were removed
        let stats = storage.get_storage_stats().await;
        assert_eq!(stats.file_count, 0);
        assert_eq!(stats.used_space, 0);
    }
} 