use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use std::time::{Duration, Instant};
use crate::error::PluginError;
use tracing::{info, warn, error, debug};

#[derive(Debug)]
pub struct Cache {
    root_path: PathBuf,
    state: Arc<RwLock<CacheState>>,
}

#[derive(Debug)]
struct CacheState {
    entries: HashMap<String, CacheEntry>,
    used_space: u64,
    max_space: u64,
}

#[derive(Debug)]
struct CacheEntry {
    path: PathBuf,
    size: u64,
    last_access: Instant,
    ttl: Duration,
}

impl Cache {
    pub fn new<P: AsRef<Path>>(root_path: P, max_space: u64) -> Self {
        info!("Initializing cache with root path: {:?}, max space: {} bytes", 
            root_path.as_ref(), max_space);
        
        let cache = Self {
            root_path: root_path.as_ref().to_owned(),
            state: Arc::new(RwLock::new(CacheState {
                entries: HashMap::new(),
                used_space: 0,
                max_space,
            })),
        };

        debug!("Cache initialized successfully");
        cache
    }

    pub async fn store(&self, key: String, data: Vec<u8>, ttl: Duration) -> Result<(), PluginError> {
        let mut state = self.state.write().await;
        let size = data.len() as u64;

        debug!("Attempting to store {} bytes for key: {}", size, key);

        // 检查空间
        if state.used_space + size > state.max_space {
            warn!("Cache space exceeded. Current: {}, Required: {}, Max: {}", 
                state.used_space, size, state.max_space);
            return Err(PluginError::Storage("Cache space exceeded".into()));
        }

        // 存储文件
        let file_path = self.root_path.join(&key);
        match tokio::fs::write(&file_path, &data).await {
            Ok(_) => {
                // 更新缓存状态
                state.entries.insert(key.clone(), CacheEntry {
                    path: file_path.clone(),
                    size,
                    last_access: Instant::now(),
                    ttl,
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

    pub async fn get(&self, key: &str) -> Result<Vec<u8>, PluginError> {
        debug!("Attempting to retrieve key: {}", key);
        let mut state = self.state.write().await;

        if let Some(entry) = state.entries.get_mut(key) {
            // 检查 TTL
            if entry.last_access.elapsed() > entry.ttl {
                warn!("Cache entry expired for key: {}", key);
                state.entries.remove(key);
                state.used_space -= entry.size;
                return Err(PluginError::Storage("Cache entry expired".into()));
            }

            // 更新访问时间
            entry.last_access = Instant::now();
            
            // 读取文件
            match tokio::fs::read(&entry.path).await {
                Ok(data) => {
                    info!("Successfully retrieved {} bytes for key: {}", data.len(), key);
                    Ok(data)
                }
                Err(e) => {
                    error!("Failed to read cache file for key {}: {}", key, e);
                    Err(PluginError::Storage(e.to_string()))
                }
            }
        } else {
            debug!("Cache miss for key: {}", key);
            Err(PluginError::Storage("Cache miss".into()))
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
            .filter(|(_, entry)| now.duration_since(entry.last_access) > entry.ttl)
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

        info!("Cache cleanup completed. Removed {} entries, freed {} bytes", 
            removed_count, freed_space);
        Ok(())
    }

    pub async fn get_stats(&self) -> Result<CacheStats, PluginError> {
        let state = self.state.read().await;
        let stats = CacheStats {
            total_entries: state.entries.len(),
            used_space: state.used_space,
            max_space: state.max_space,
            usage_percent: (state.used_space as f64 / state.max_space as f64) * 100.0,
        };
        debug!("Cache stats: {:?}", stats);
        Ok(stats)
    }
}

#[derive(Debug)]
pub struct CacheStats {
    pub total_entries: usize,
    pub used_space: u64,
    pub max_space: u64,
    pub usage_percent: f64,
} 