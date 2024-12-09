use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{info, warn, error, debug};
use tokio::io::AsyncWriteExt;
use serde::{Serialize, Deserialize};

use crate::error::PluginError;

mod entry;
mod metadata;

pub use metadata::CacheMetadata;
use entry::CacheEntry;

#[derive(Debug)]
pub struct CacheManager {
    root_path: PathBuf,
    state: Arc<RwLock<CacheState>>,
    config: CacheConfig,
}

impl Clone for CacheManager {
    fn clone(&self) -> Self {
        Self {
            root_path: self.root_path.clone(),
            state: self.state.clone(),
            config: self.config.clone(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct CacheState {
    entries: HashMap<String, CacheEntry>,
    used_space: u64,
    max_space: u64,
}

#[derive(Debug, Clone)]
pub struct CacheConfig {
    pub max_space: u64,
    pub entry_ttl: Duration,
    pub min_free_space: u64,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            max_space: 1024 * 1024 * 1024, // 1GB
            entry_ttl: Duration::from_secs(86400), // 24 hours
            min_free_space: 1024 * 1024 * 100, // 100MB
        }
    }
}

impl CacheManager {
    pub fn new<P: AsRef<Path>>(root_path: P, config: CacheConfig) -> Self {
        info!("Initializing cache manager with root path: {:?}", root_path.as_ref());
        debug!("Cache config: {:?}", config);

        let root_path = root_path.as_ref().to_owned();
        
        // 确保缓存目录存在
        tokio::task::block_in_place(|| {
            std::fs::create_dir_all(&root_path)
                .unwrap_or_else(|e| warn!("Failed to create cache directory: {}", e));
        });

        // 尝试加载缓存状态
        let state = Self::load_state(&root_path).unwrap_or_else(|e| {
            warn!("Failed to load cache state: {}, creating new state", e);
            CacheState {
                entries: HashMap::new(),
                used_space: 0,
                max_space: config.max_space,
            }
        });

        info!("Loaded cache state: {} entries, {} bytes used", 
            state.entries.len(), state.used_space);

        // 验证缓存文件
        let state = Self::verify_cache_files(state, &root_path);
        
        let cache = Self {
            root_path,
            state: Arc::new(RwLock::new(state)),
            config,
        };

        // 启动定期保存状态的任务
        let cache_clone = Arc::new(cache.clone());
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(60)).await;
                if let Err(e) = cache_clone.save_state().await {
                    error!("Failed to save cache state: {}", e);
                }
            }
        });

        cache
    }

    fn load_state(root_path: &Path) -> Result<CacheState, PluginError> {
        let state_file = root_path.join("cache_state.json");
        if !state_file.exists() {
            return Err(PluginError::Storage("State file not found".into()));
        }

        let content = std::fs::read_to_string(&state_file)
            .map_err(|e| PluginError::Storage(format!("Failed to read state file: {}", e)))?;

        serde_json::from_str(&content)
            .map_err(|e| PluginError::Storage(format!("Failed to parse state file: {}", e)))
    }

    fn verify_cache_files(mut state: CacheState, root_path: &Path) -> CacheState {
        let mut verified_entries = HashMap::new();
        let mut total_size = 0;

        for (key, entry) in state.entries {
            if entry.path.exists() {
                match std::fs::metadata(&entry.path) {
                    Ok(metadata) => {
                        let actual_size = metadata.len();
                        if actual_size != entry.size {
                            warn!("Cache file size mismatch for {}: expected {}, actual {}", 
                                key, entry.size, actual_size);
                            // 更新为实际大小
                            let mut entry_clone = entry.clone();
                            entry_clone.size = actual_size;
                            verified_entries.insert(key, entry_clone);
                            total_size += actual_size;
                        } else {
                            verified_entries.insert(key.clone(), entry.clone());
                            total_size += entry.size;
                        }
                    }
                    Err(e) => {
                        warn!("Failed to read cache file metadata for {}: {}", key, e);
                        continue;
                    }
                }
            } else {
                warn!("Cache file missing for key: {}, path: {:?}", key, entry.path);
                continue;
            }
        }

        CacheState {
            entries: verified_entries,
            used_space: total_size,
            max_space: state.max_space,
        }
    }

    async fn save_state(&self) -> Result<(), PluginError> {
        let state = self.state.read().await;
        let state_file = self.root_path.join("cache_state.json");
        let temp_file = state_file.with_extension("tmp");

        // 先写入临时文件
        let content = serde_json::to_string_pretty(&*state)
            .map_err(|e| PluginError::Storage(format!("Failed to serialize state: {}", e)))?;

        tokio::fs::write(&temp_file, content).await
            .map_err(|e| PluginError::Storage(format!("Failed to write state file: {}", e)))?;

        // 原子地重命名
        tokio::fs::rename(&temp_file, &state_file).await
            .map_err(|e| PluginError::Storage(format!("Failed to save state file: {}", e)))?;

        debug!("Cache state saved successfully");
        Ok(())
    }

    pub async fn store(&self, key: String, data: Vec<u8>, metadata: CacheMetadata) -> Result<(), PluginError> {
        debug!("Attempting to store {} bytes for key: {}", data.len(), key);
        let mut state = self.state.write().await;

        // 检查空间
        let size = data.len() as u64;
        if state.used_space + size > state.max_space {
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
        
        // 先检查文件和条目是否存在，并获取必要信息
        let cache_info = {
            let mut state = self.state.write().await;
            
            // 先获取条目的引用
            let entry = state.entries.get(key);  // 使用 get 而不是 get_mut
            
            match entry {
                Some(entry) => {
                    debug!("Found cache entry for key: {}, size: {}, path: {:?}", 
                        key, entry.size, entry.path);
                    
                    // 检查文件是否存在
                    if !entry.path.exists() {
                        warn!("Cache file missing for key: {}, path: {:?}", key, entry.path);
                        let size = entry.size;
                        let path = entry.path.clone();
                        // 删除条目并更新空间
                        state.entries.remove(key);
                        state.used_space -= size;
                        return Err(PluginError::Storage(format!("Cache file missing: {:?}", path)));
                    }

                    // 检查文件权限
                    match std::fs::metadata(&entry.path) {
                        Ok(metadata) => {
                            debug!("Cache file metadata: readonly={}, size={}, modified={:?}", 
                                metadata.permissions().readonly(),
                                metadata.len(),
                                metadata.modified().ok());
                        }
                        Err(e) => {
                            warn!("Failed to read cache file metadata: {}, path: {:?}", e, entry.path);
                        }
                    }

                    // 检查 TTL
                    if entry.is_expired(self.config.entry_ttl) {
                        warn!("Cache entry expired for key: {}, last_access: {:?}", 
                            key, entry.last_access);
                        let path = entry.path.clone();
                        let size = entry.size;
                        // 删除条目并更新空间
                        state.entries.remove(key);
                        state.used_space -= size;
                        
                        // 删除过期文件
                        if let Err(e) = tokio::fs::remove_file(&path).await {
                            warn!("Failed to remove expired cache file: {}, path: {:?}", e, path);
                        }
                        
                        return Err(PluginError::Storage("Cache entry expired".into()));
                    }

                    // 克隆需要的数据
                    let info = CacheInfo {
                        path: entry.path.clone(),
                        metadata: entry.metadata.clone(),
                        size: entry.size,
                    };

                    // 更新访问时间
                    if let Some(entry) = state.entries.get_mut(key) {
                        entry.touch();
                        debug!("Updated last_access time for key: {}", key);
                    }

                    info
                }
                None => {
                    debug!("Cache miss for key: {}, no entry found in cache state", key);
                    return Err(PluginError::Storage("Cache miss".into()));
                }
            }
        };

        // 读取文件
        debug!("Reading cache file: {:?}", cache_info.path);
        match tokio::fs::read(&cache_info.path).await {
            Ok(data) => {
                info!("Successfully retrieved {} bytes for key: {} from {:?}", 
                    data.len(), key, cache_info.path);
                Ok((data, cache_info.metadata))
            }
            Err(e) => {
                error!("Failed to read cache file for key {}: {}, path: {:?}", 
                    key, e, cache_info.path);
                // 如果文件读取失败，清理缓存条目
                let mut state = self.state.write().await;
                if let Some(entry) = state.entries.remove(key) {
                    state.used_space -= entry.size;
                    warn!("Removed cache entry due to read failure, freed {} bytes", entry.size);
                }
                Err(PluginError::Storage(format!("Failed to read cache file: {}", e)))
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
            last_cleanup: Instant::now(),
        };
        debug!("Cache stats: {:?}", stats);
        stats
    }

    pub async fn start_cleanup_task(self: Arc<Self>) {
        info!("Starting cache cleanup task");
        let cleanup_interval = self.config.entry_ttl;

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

    pub async fn store_stream(&self, key: String, metadata: CacheMetadata) -> Result<(), PluginError> {
        info!("CACHE: Creating stream for key: {}", key);
        let file_path = self.root_path.join(&key);
        
        // 确保父目录存在
        if let Some(parent) = file_path.parent() {
            info!("CACHE: Creating directory: {:?}", parent);
            tokio::fs::create_dir_all(parent).await
                .map_err(|e| {
                    error!("CACHE: Failed to create directory {:?}: {}", parent, e);
                    PluginError::Storage(format!("Failed to create directory: {}", e))
                })?;
        }

        // 创建文件
        info!("CACHE: Creating file: {:?}", file_path);
        match tokio::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&file_path)
            .await
        {
            Ok(_) => {
                info!("CACHE: Successfully created file: {:?}", file_path);
            }
            Err(e) => {
                error!("CACHE: Failed to create file {:?}: {}", file_path, e);
                return Err(PluginError::Storage(format!("Failed to create file: {}", e)));
            }
        }

        // 添加缓存条目
        let mut state = self.state.write().await;
        state.entries.insert(key.clone(), CacheEntry {
            path: file_path.clone(),
            size: 0,
            last_access: Instant::now(),
            metadata,
        });
        info!("CACHE: Added new cache entry for key: {}", key);

        Ok(())
    }

    pub async fn append_chunk(&self, key: &str, chunk: &[u8]) -> Result<(), PluginError> {
        debug!("Appending {} bytes to cache key: {}", chunk.len(), key);

        // First, get the entry path and check space without holding a mutable reference
        let (entry_path, max_space, current_used_space) = {
            let state = self.state.read().await;
            match state.entries.get(key) {
                Some(entry) => (entry.path.clone(), self.config.max_space, state.used_space),
                None => return Err(PluginError::Storage("Cache entry not found".into())),
            }
        };

        // Check space
        let chunk_size = chunk.len() as u64;
        if current_used_space + chunk_size > max_space {
            warn!("Cache space exceeded while appending chunk");
            return Err(PluginError::Storage("Cache space exceeded".into()));
        }

        debug!("Appending {} bytes to cache file for key: {}", chunk_size, key);

        // Write to the file
        let mut file = tokio::fs::OpenOptions::new()
            .append(true)
            .open(&entry_path)
            .await
            .map_err(|e| PluginError::Storage(format!("Failed to open file: {}", e)))?;

        file.write_all(chunk).await
            .map_err(|e| PluginError::Storage(format!("Failed to write chunk: {}", e)))?;

        // Update state with a new mutable reference
        let mut state = self.state.write().await;
        if let Some(entry) = state.entries.get_mut(key) {
            entry.size += chunk_size;
            state.used_space += chunk_size;
        }

        Ok(())
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

// 添加一个辅助结构体来存储缓存信息
#[derive(Debug)]
struct CacheInfo {
    path: PathBuf,
    metadata: CacheMetadata,
    size: u64,
} 