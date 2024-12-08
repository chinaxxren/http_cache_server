use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use std::time::{Duration, Instant};
use crate::error::PluginError;
use tracing::{info, warn, error, debug};

mod cleaner;
mod policy;
mod entry;
mod metadata;

pub use cleaner::CacheCleaner;
pub use policy::CachePolicy;
pub use metadata::CacheMetadata;
use entry::CacheEntry;

#[derive(Debug)]
pub struct CacheManager {
    root_path: PathBuf,
    state: Arc<RwLock<CacheState>>,
    config: CacheConfig,
}

#[derive(Debug)]
struct CacheState {
    entries: HashMap<String, CacheEntry>,
    used_space: u64,
    max_space: u64,
    last_cleanup: Instant,
}

#[derive(Debug)]
pub struct CacheConfig {
    pub max_space: u64,
    pub cleanup_interval: Duration,
    pub entry_ttl: Duration,
    pub min_free_space: u64,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            max_space: 1024 * 1024 * 1024, // 1GB
            cleanup_interval: Duration::from_secs(3600), // 1 hour
            entry_ttl: Duration::from_secs(86400), // 24 hours
            min_free_space: 1024 * 1024 * 100, // 100MB
        }
    }
}

impl CacheManager {
    pub fn new<P: AsRef<Path>>(root_path: P, config: CacheConfig) -> Self {
        info!("Initializing cache manager with root path: {:?}", root_path.as_ref());
        debug!("Cache config: {:?}", config);

        Self {
            root_path: root_path.as_ref().to_owned(),
            state: Arc::new(RwLock::new(CacheState {
                entries: HashMap::new(),
                used_space: 0,
                max_space: config.max_space,
                last_cleanup: Instant::now(),
            })),
            config,
        }
    }

    pub async fn store(&self, key: String, data: Vec<u8>, metadata: CacheMetadata) -> Result<(), PluginError> {
        debug!("Attempting to store {} bytes for key: {}", data.len(), key);
        let mut state = self.state.write().await;

        // 检查空间
        let size = data.len() as u64;
        if state.used_space + size > state.max_space {
            warn!("Cache space exceeded. Current: {}, Required: {}, Max: {}", 
                state.used_space, size, state.max_space);
            return Err(PluginError::Storage("Cache space exceeded".into()));
        }

        // 存储文件
        let file_path = self.root_path.join(&key);
        match tokio::fs::write(&file_path, &data).await {
            Ok(_) => {
                state.entries.insert(key.clone(), CacheEntry {
                    path: file_path.clone(),
                    size,
                    last_access: Instant::now(),
                    metadata,
                });
                state.used_space += size;
                info!("Successfully stored {} bytes for key: {}", size, key);
                Ok(())
            }
            Err(e) => {
                error!("Failed to store data for key {}: {}", key, e);
                Err(PluginError::Storage(e.to_string()))
            }
        }
    }

    pub async fn get(&self, key: &str) -> Result<(Vec<u8>, CacheMetadata), PluginError> {
        debug!("Attempting to retrieve key: {}", key);
        let mut state = self.state.write().await;

        let entry = match state.entries.get_mut(key) {
            Some(entry) => entry,
            None => {
                debug!("Cache miss for key: {}", key);
                return Err(PluginError::Storage("Cache miss".into()));
            }
        };

        // 检查 TTL
        if entry.is_expired(self.config.entry_ttl) {
            warn!("Cache entry expired for key: {}", key);
            let size = entry.size;
            state.entries.remove(key);
            state.used_space -= size;
            return Err(PluginError::Storage("Cache entry expired".into()));
        }

        // 更新访问时间
        entry.touch();
        let metadata = entry.metadata.clone();
        let path = entry.path.clone();
        
        // 读取文件
        match tokio::fs::read(&path).await {
            Ok(data) => {
                info!("Successfully retrieved {} bytes for key: {}", data.len(), key);
                Ok((data, metadata))
            }
            Err(e) => {
                error!("Failed to read cache file for key {}: {}", key, e);
                Err(PluginError::Storage(e.to_string()))
            }
        }
    }

    pub async fn cleanup(&self) -> Result<(), PluginError> {
        info!("Starting cache cleanup");
        let mut state = self.state.write().await;
        let now = Instant::now();
        let mut removed_count = 0;
        let mut freed_space = 0;

        // 找出过期条目
        let expired: Vec<_> = state.entries.iter()
            .filter(|(_, entry)| now.duration_since(entry.last_access) > self.config.entry_ttl)
            .map(|(k, _)| k.clone())
            .collect();

        debug!("Found {} expired entries", expired.len());

        // 删除过期条目
        for key in expired {
            if let Some(entry) = state.entries.remove(&key) {
                match tokio::fs::remove_file(&entry.path).await {
                    Ok(_) => {
                        state.used_space -= entry.size;
                        freed_space += entry.size;
                        removed_count += 1;
                        debug!("Removed expired entry: {}", key);
                    }
                    Err(e) => warn!("Failed to remove cache file {}: {}", key, e),
                }
            }
        }

        state.last_cleanup = now;
        info!("Cache cleanup completed. Removed {} entries, freed {} bytes", 
            removed_count, freed_space);
        Ok(())
    }

    pub async fn get_stats(&self) -> CacheStats {
        let state = self.state.read().await;
        let stats = CacheStats {
            total_entries: state.entries.len(),
            used_space: state.used_space,
            max_space: state.max_space,
            usage_percent: (state.used_space as f64 / state.max_space as f64) * 100.0,
            last_cleanup: state.last_cleanup,
        };
        debug!("Cache stats: {:?}", stats);
        stats
    }

    pub async fn start_cleanup_task(self: Arc<Self>) {
        info!("Starting cache cleanup task");
        let cleanup_interval = self.config.cleanup_interval;

        tokio::spawn(async move {
            loop {
                tokio::time::sleep(cleanup_interval).await;
                debug!("Running scheduled cleanup");
                if let Err(e) = self.cleanup().await {
                    error!("Error during cleanup: {}", e);
                }
            }
        });
    }
}

#[derive(Debug)]
pub struct CacheStats {
    pub total_entries: usize,
    pub used_space: u64,
    pub max_space: u64,
    pub usage_percent: f64,
    pub last_cleanup: Instant,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::test;

    #[test]
    async fn test_basic_cache_operations() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cache = CacheManager::new(
            temp_dir.path(),
            CacheConfig::default(),
        );

        // Store data
        let data = b"test data".to_vec();
        let metadata = CacheMetadata {
            content_type: "text/plain".to_string(),
            etag: Some("123".to_string()),
            last_modified: None,
            created_at: chrono::Utc::now(),
        };

        cache.store("test.txt".to_string(), data.clone(), metadata.clone())
            .await
            .unwrap();

        // Retrieve data
        let (retrieved_data, retrieved_metadata) = cache.get("test.txt")
            .await
            .unwrap();

        assert_eq!(retrieved_data, data);
        assert_eq!(retrieved_metadata.content_type, "text/plain");
        assert_eq!(retrieved_metadata.etag, Some("123".to_string()));
    }

    #[test]
    async fn test_cache_cleanup() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cache = CacheManager::new(
            temp_dir.path(),
            CacheConfig {
                entry_ttl: Duration::from_secs(1),
                ..Default::default()
            },
        );

        // Store some entries
        let data = b"test data".to_vec();
        let metadata = CacheMetadata {
            content_type: "text/plain".to_string(),
            etag: None,
            last_modified: None,
            created_at: chrono::Utc::now(),
        };

        cache.store("file1.txt".to_string(), data.clone(), metadata.clone())
            .await
            .unwrap();

        // Wait for entries to expire
        tokio::time::sleep(Duration::from_secs(2)).await;

        // Run cleanup
        cache.cleanup().await.unwrap();

        // Verify cleanup
        let stats = cache.get_stats().await;
        assert_eq!(stats.total_entries, 0);
        assert_eq!(stats.used_space, 0);
    }
} 