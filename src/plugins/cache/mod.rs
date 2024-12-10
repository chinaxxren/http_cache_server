pub mod error;

use std::path::{Path, PathBuf};
use std::time::Duration;
use std::collections::HashMap;
use tokio::sync::RwLock;
use serde::{Serialize, Deserialize};
use chrono::{DateTime, Utc};
use tracing::{info, warn, error};
use std::time::Instant;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;

use crate::error::PluginError;
pub use self::error::CacheError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheMetadata {
    pub content_type: String,
    pub last_modified: Option<DateTime<Utc>>,
    pub etag: Option<String>,
}

impl CacheMetadata {
    pub fn new(content_type: String) -> Self {
        Self {
            content_type,
            last_modified: None,
            etag: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry {
    pub path: PathBuf,
    pub size: u64,
    pub last_access: DateTime<Utc>,
    pub metadata: CacheMetadata,
    pub expires_at: Option<DateTime<Utc>>,
}

impl CacheEntry {
    fn new(path: PathBuf, size: u64, metadata: CacheMetadata, ttl: Option<Duration>) -> Self {
        let expires_at = ttl.map(|d| Utc::now() + chrono::Duration::from_std(d).unwrap());
        Self {
            path,
            size,
            last_access: Utc::now(),
            metadata,
            expires_at,
        }
    }

    fn is_expired(&self) -> bool {
        if let Some(expires_at) = self.expires_at {
            Utc::now() > expires_at
        } else {
            false
        }
    }
}

#[derive(Debug)]
pub struct CacheConfig {
    pub max_space: u64,        // 最大缓存空间(字节)
    pub entry_ttl: Duration,   // 缓存项有效期
    pub min_free_space: u64,   // 最小剩余空间(字节)
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            max_space: 1024 * 1024 * 1024,  // 1GB
            entry_ttl: Duration::from_secs(3600 * 24),  // 24小时
            min_free_space: 1024 * 1024 * 100,  // 100MB
        }
    }
}

impl CacheConfig {
    pub fn validate(&self) -> Result<(), CacheError> {
        if self.max_space < self.min_free_space {
            return Err(CacheError::Other(
                "max_space must be greater than min_free_space".into()
            ));
        }

        if self.entry_ttl.as_secs() == 0 {
            return Err(CacheError::Other(
                "entry_ttl must be greater than zero".into()
            ));
        }

        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct CacheState {
    total_size: u64,
    used_size: u64,
    max_size: u64,
    entries: HashMap<String, CacheEntry>,
}

pub struct CacheManager {
    base_path: PathBuf,
    config: CacheConfig,
    state: RwLock<CacheState>,
}

impl CacheManager {
    pub fn new<P: AsRef<Path>>(base_path: P, config: CacheConfig) -> Self {
        // 创建缓存目录
        std::fs::create_dir_all(&base_path).unwrap_or_else(|e| {
            error!("Failed to create cache directory: {}", e);
        });

        // 加载状态
        let state = if let Ok(content) = std::fs::read_to_string(base_path.as_ref().join("cache_state.json")) {
            serde_json::from_str(&content).unwrap_or_else(|e| {
                warn!("Failed to parse cache state: {}", e);
                CacheState {
                    total_size: 0,
                    used_size: 0,
                    max_size: config.max_space,
                    entries: HashMap::new(),
                }
            })
        } else {
            CacheState {
                total_size: 0,
                used_size: 0,
                max_size: config.max_space,
                entries: HashMap::new(),
            }
        };

        Self {
            base_path: base_path.as_ref().to_path_buf(),
            config,
            state: RwLock::new(state),
        }
    }

    pub async fn get(&self, key: &str) -> Result<(Vec<u8>, CacheMetadata), CacheError> {
        let mut state = self.state.write().await;
        if let Some(entry) = state.entries.get(key) {
            // 检查是否过期
            if entry.is_expired() {
                // 删除过期内容
                if let Err(e) = tokio::fs::remove_file(&entry.path).await {
                    warn!("Failed to remove expired cache file {}: {}", entry.path.display(), e);
                }
                state.entries.remove(key);
                return Err(CacheError::NotFound(key.to_string()));
            }

            // 更新访问时间
            if let Some(entry) = state.entries.get_mut(key) {
                entry.last_access = Utc::now();
                let content = tokio::fs::read(&entry.path).await
                    .map_err(|e| CacheError::IO(format!("Failed to read cache file: {}", e)))?;
                Ok((content, entry.metadata.clone()))
            } else {
                Err(CacheError::NotFound(key.to_string()))
            }
        } else {
            Err(CacheError::NotFound(key.to_string()))
        }
    }

    pub async fn store(&self, key: String, content: Vec<u8>, metadata: CacheMetadata) -> Result<(), CacheError> {
        let size = content.len() as u64;
        
        // 检查空间
        self.ensure_space(size).await?;

        // 生成文件路径
        let path = self.base_path.join(format!("{}.cache", key));
        self.validate_path(&path)?;

        // 写入文件
        tokio::fs::write(&path, &content).await
            .map_err(|e| CacheError::IO(format!("Failed to write cache file: {}", e)))?;

        // 更新状态
        let mut state = self.state.write().await;
        state.entries.insert(key, CacheEntry {
            path,
            size,
            last_access: Utc::now(),
            metadata,
            expires_at: None,
        });
        state.used_size += size;

        // 保存状态
        drop(state);
        self.save_state().await?;

        Ok(())
    }

    async fn cleanup_space(&self, needed_space: u64) -> Result<(), CacheError> {
        let mut state = self.state.write().await;
        
        // 先收集要删除的条目
        let mut to_remove = Vec::new();
        let mut freed_space = 0;
        
        // 按最后访问时间排序
        let mut entries: Vec<_> = state.entries.iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        entries.sort_by(|a, b| a.1.last_access.cmp(&b.1.last_access));

        // 标记要删除的条目
        for (key, entry) in entries {
            if state.used_size - freed_space + needed_space <= self.config.max_space {
                break;
            }

            // 删除文件
            if let Err(e) = tokio::fs::remove_file(&entry.path).await {
                warn!("Failed to remove cache file {}: {}", entry.path.display(), e);
            }

            to_remove.push(key);
            freed_space += entry.size;
        }

        // 批量删除条目
        for key in to_remove {
            state.entries.remove(&key);
        }

        state.used_size -= freed_space;
        self.save_state().await?;

        Ok(())
    }

    async fn save_state(&self) -> Result<(), CacheError> {
        let state = self.state.read().await;
        let content = serde_json::to_string_pretty(&*state)
            .map_err(|e| CacheError::Storage(format!("Failed to serialize state: {}", e)))?;

        let temp_path = self.base_path.join("cache_state.json.tmp");
        let final_path = self.base_path.join("cache_state.json");

        tokio::fs::write(&temp_path, &content).await
            .map_err(|e| CacheError::IO(format!("Failed to write state file: {}", e)))?;

        tokio::fs::rename(temp_path, final_path).await
            .map_err(|e| CacheError::IO(format!("Failed to rename state file: {}", e)))?;

        Ok(())
    }

    pub async fn cleanup(&self) -> Result<(), CacheError> {
        let mut state = self.state.write().await;
        let now = Utc::now();
        let mut to_remove = Vec::new();
        let mut freed_space = 0;

        // 检查过期和无效的条目
        for (key, entry) in state.entries.iter() {
            // 检查 TTL
            let age = now - entry.last_access;
            if age > chrono::Duration::from_std(self.config.entry_ttl).unwrap() {
                to_remove.push(key.clone());
                freed_space += entry.size;
                continue;
            }

            // 检查文件是否存在
            if !entry.path.exists() {
                to_remove.push(key.clone());
                freed_space += entry.size;
                continue;
            }
        }

        // 删除过期条目
        for key in to_remove {
            if let Some(entry) = state.entries.remove(&key) {
                if let Err(e) = tokio::fs::remove_file(&entry.path).await {
                    warn!("Failed to remove cache file {}: {}", entry.path.display(), e);
                }
            }
        }

        // 更新使用空间
        state.used_size -= freed_space;

        // 保存状��
        drop(state);
        self.save_state().await?;

        Ok(())
    }

    pub async fn get_stats(&self) -> CacheStats {
        let state = self.state.read().await;
        CacheStats {
            total_entries: state.entries.len(),
            used_space: state.used_size,
            max_space: state.max_size,
            usage_percent: (state.used_size as f64 / state.max_size as f64) * 100.0,
            last_cleanup: Instant::now(),
        }
    }

    pub async fn clear(&self) -> Result<(), CacheError> {
        let mut state = self.state.write().await;
        
        // 删除所有缓存文件
        for entry in state.entries.values() {
            if let Err(e) = tokio::fs::remove_file(&entry.path).await {
                warn!("Failed to remove cache file {}: {}", entry.path.display(), e);
            }
        }

        // 清空状态
        state.entries.clear();
        state.used_size = 0;

        // 保存状态
        drop(state);
        self.save_state().await?;

        Ok(())
    }

    pub async fn remove(&self, key: &str) -> Result<(), CacheError> {
        let mut state = self.state.write().await;
        
        if let Some(entry) = state.entries.remove(key) {
            // 删除文件
            if let Err(e) = tokio::fs::remove_file(&entry.path).await {
                warn!("Failed to remove cache file {}: {}", entry.path.display(), e);
            }
            
            // 更新使用空间
            state.used_size -= entry.size;

            // 保存状态
            drop(state);
            self.save_state().await?;
        }

        Ok(())
    }

    pub async fn contains(&self, key: &str) -> bool {
        let state = self.state.read().await;
        state.entries.contains_key(key)
    }

    pub async fn get_metadata(&self, key: &str) -> Option<CacheMetadata> {
        let state = self.state.read().await;
        state.entries.get(key).map(|e| e.metadata.clone())
    }

    pub fn validate_config(&self) -> Result<(), CacheError> {
        if self.config.max_space < self.config.min_free_space {
            return Err(CacheError::Other(
                "max_space must be greater than min_free_space".into()
            ));
        }

        if self.config.entry_ttl.as_secs() == 0 {
            return Err(CacheError::Other(
                "entry_ttl must be greater than zero".into()
            ));
        }

        Ok(())
    }

    fn validate_path(&self, path: &Path) -> Result<(), CacheError> {
        // 检查路径是否在缓存目录内
        if !path.starts_with(&self.base_path) {
            return Err(CacheError::InvalidPath(
                format!("Path must be inside cache directory: {}", path.display())
            ));
        }

        // 检查路径是否包含 ..
        if path.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
            return Err(CacheError::InvalidPath(
                format!("Path must not contain ..: {}", path.display())
            ));
        }

        Ok(())
    }

    async fn ensure_space(&self, needed: u64) -> Result<(), CacheError> {
        let state = self.state.read().await;
        let available = self.config.max_space - state.used_size;
        
        if needed > available {
            return Err(CacheError::Full {
                needed,
                available,
            });
        }
        Ok(())
    }

    /// 创建流式缓存
    pub async fn store_stream(&self, key: String, metadata: CacheMetadata) -> Result<(), CacheError> {
        // 生成文件路径
        let path = self.base_path.join(format!("{}.cache", key));
        self.validate_path(&path)?;

        // 创建目录
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        // 创建文件
        File::create(&path).await?;

        // 更新状态
        let mut state = self.state.write().await;
        state.entries.insert(key, CacheEntry {
            path,
            size: 0,
            last_access: Utc::now(),
            metadata,
            expires_at: None,
        });

        // 保存状态
        drop(state);
        self.save_state().await?;

        Ok(())
    }

    /// 追加数据块到流式缓存
    pub async fn append_chunk(&self, key: &str, chunk: &[u8]) -> Result<(), CacheError> {
        let chunk_size = chunk.len() as u64;
        
        // 检查空间
        self.ensure_space(chunk_size).await?;

        // 获取文件路径
        let mut state = self.state.write().await;
        let entry = state.entries.get_mut(key)
            .ok_or_else(|| CacheError::NotFound(key.to_string()))?;

        // 追加数据
        let mut file = File::options()
            .append(true)
            .open(&entry.path)
            .await?;

        file.write_all(chunk).await?;
        file.flush().await?;

        // 更新状态
        entry.size += chunk_size;
        state.used_size += chunk_size;

        // 保存状态
        drop(state);
        self.save_state().await?;

        Ok(())
    }

    /// 完成流式缓存
    pub async fn finish_stream(&self, key: &str) -> Result<(), CacheError> {
        let mut state = self.state.write().await;
        if let Some(entry) = state.entries.get_mut(key) {
            entry.last_access = Utc::now();
            // 可以在这里添加其他完成时的处理
        }
        Ok(())
    }

    /// 存储带 TTL 的缓存内容
    pub async fn store_with_ttl(
        &self,
        key: String,
        data: Vec<u8>,
        metadata: CacheMetadata,
        ttl: Duration
    ) -> Result<(), CacheError> {
        let size = data.len() as u64;
        
        // 检查空间
        self.ensure_space(size).await?;

        // 生成文件路径
        let path = self.base_path.join(format!("{}.cache", key));
        self.validate_path(&path)?;

        // 写入文件
        tokio::fs::write(&path, &data).await
            .map_err(|e| CacheError::IO(format!("Failed to write cache file: {}", e)))?;

        // 创建缓存条目
        let entry = CacheEntry::new(path, size, metadata, Some(ttl));

        // 更新状态
        let mut state = self.state.write().await;
        state.used_size += size;
        state.entries.insert(key, entry);
        
        // 保存状态
        drop(state);
        self.save_state().await?;

        Ok(())
    }
}

impl std::fmt::Debug for CacheManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CacheManager")
            .field("base_path", &self.base_path)
            .field("config", &self.config)
            .finish()
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