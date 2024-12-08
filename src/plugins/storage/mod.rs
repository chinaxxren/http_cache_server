use crate::plugin::Plugin;
use crate::error::PluginError;
use async_trait::async_trait;
use std::path::PathBuf;
use tokio::sync::RwLock;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tracing::{info, warn, error, debug};

/// 存储插件，提供文件存储和管理功能
/// 
/// # 功能
/// - 文件存储和检索
/// - 空间管理
/// - 自动清理过期文件
/// - 存储统计
#[derive(Debug)]
pub struct StoragePlugin {
    /// 存储根目录路径
    root_path: PathBuf,
    /// 存储状态，包含文件信息和使用统计
    state: RwLock<StorageState>,
}

/// 存储插件的运行时状态
#[derive(Debug)]
struct StorageState {
    /// 已使用的存储空间（字节）
    used_space: u64,
    /// 最大允许使用的存储空间（字节）
    max_space: u64,
    /// 存储的文件信息映射表
    files: HashMap<String, FileInfo>,
}

/// 单个文件的存储信息
#[derive(Debug)]
struct FileInfo {
    /// 文件在磁盘上的路径
    path: PathBuf,
    /// 文件大小（字节）
    size: u64,
    /// 最后访问时间
    last_access: Instant,
}

impl StoragePlugin {
    /// 创建新的存储插件实例
    /// 
    /// # 参数
    /// - `root_path`: 存储根目录路径
    /// - `max_space`: 最大允许使用的存储空间（字节）
    /// 
    /// # 示例
    /// ```
    /// use http_cache_server::plugins::storage::StoragePlugin;
    /// use std::path::PathBuf;
    /// 
    /// let storage = StoragePlugin::new(
    ///     PathBuf::from("./storage"),
    ///     1024 * 1024 * 1024 // 1GB
    /// );
    /// ```
    pub fn new(root_path: PathBuf, max_space: u64) -> Self {
        info!("Initializing StoragePlugin with root path: {:?}, max space: {} bytes", 
            root_path, max_space);
        Self {
            root_path,
            state: RwLock::new(StorageState {
                used_space: 0,
                max_space,
                files: HashMap::new(),
            }),
        }
    }

    /// 存储文件
    /// 
    /// # 参数
    /// - `key`: 文件的唯一标识符
    /// - `data`: 文件数据
    /// 
    /// # 错误
    /// - 如果存储空间不足
    /// - 如果写入文件失败
    pub async fn store_file(&self, key: String, data: Vec<u8>) -> Result<(), PluginError> {
        let mut state = self.state.write().await;
        let file_size = data.len() as u64;

        debug!("Storing file: {} ({} bytes)", key, file_size);

        // 检查空间
        if state.used_space + file_size > state.max_space {
            warn!("Storage space exceeded. Current: {}, Required: {}, Max: {}", 
                state.used_space, file_size, state.max_space);
            return Err(PluginError::Storage("Storage space exceeded".into()));
        }

        // 存储文件
        let file_path = self.root_path.join(&key);
        match tokio::fs::write(&file_path, data).await {
            Ok(_) => {
                // 更新状态
                state.files.insert(key.clone(), FileInfo {
                    path: file_path,
                    size: file_size,
                    last_access: Instant::now(),
                });
                state.used_space += file_size;
                info!("File stored successfully: {}", key);
                Ok(())
            }
            Err(e) => {
                error!("Failed to store file {}: {}", key, e);
                Err(PluginError::Storage(e.to_string()))
            }
        }
    }

    /// 获取文件内容
    /// 
    /// # 参数
    /// - `key`: 文件的唯一标识符
    /// 
    /// # 错误
    /// - 如果文件不存在
    /// - 如果读取文件失败
    pub async fn get_file(&self, key: &str) -> Result<Vec<u8>, PluginError> {
        debug!("Attempting to retrieve file: {}", key);
        let mut state = self.state.write().await;
        if let Some(file_info) = state.files.get_mut(key) {
            file_info.last_access = Instant::now();
            match tokio::fs::read(&file_info.path).await {
                Ok(data) => {
                    info!("Successfully retrieved file: {} ({} bytes)", key, data.len());
                    Ok(data)
                }
                Err(e) => {
                    error!("Failed to read file {}: {}", key, e);
                    Err(PluginError::Storage(e.to_string()))
                }
            }
        } else {
            warn!("File not found in storage: {}", key);
            Err(PluginError::Storage("File not found".into()))
        }
    }

    /// 获取存储使用统计
    /// 
    /// # 返回
    /// - `used_space`: 已使用空间（字节）
    /// - `max_space`: 最大空间（字节）
    /// - `file_count`: 文件数量
    pub async fn get_usage_stats(&self) -> Result<(u64, u64, usize), PluginError> {
        let state = self.state.read().await;
        let stats = (
            state.used_space,
            state.max_space,
            state.files.len()
        );
        debug!("Storage stats - Used: {} bytes, Max: {} bytes, Files: {}", 
            stats.0, stats.1, stats.2);
        Ok(stats)
    }

    /// 清理过期文件
    /// 
    /// 删除超过指定时间未访问的文件
    pub async fn cleanup_expired_files(&self) -> Result<(), PluginError> {
        info!("Starting file cleanup");
        let mut state = self.state.write().await;
        let now = Instant::now();
        let mut expired_count = 0;
        let mut freed_space = 0;

        debug!("Starting expired files cleanup");
        let expired: Vec<_> = state.files.iter()
            .filter(|(_, info)| now.duration_since(info.last_access) > Duration::from_secs(3600))
            .map(|(k, _)| k.clone())
            .collect();

        for key in expired {
            if let Some(info) = state.files.remove(&key) {
                match tokio::fs::remove_file(&info.path).await {
                    Ok(_) => {
                        state.used_space -= info.size;
                        freed_space += info.size;
                        expired_count += 1;
                        debug!("Removed expired file: {}", key);
                    }
                    Err(e) => {
                        error!("Failed to remove expired file {}: {}", key, e);
                    }
                }
            }
        }

        info!("Cleanup completed: removed {} files, freed {} bytes", 
            expired_count, freed_space);
        Ok(())
    }

    pub async fn get_storage_metrics(&self) -> Result<StorageMetrics, PluginError> {
        let state = self.state.read().await;
        let metrics = StorageMetrics {
            total_space: state.max_space,
            used_space: state.used_space,
            file_count: state.files.len(),
            oldest_file: state.files.values()
                .min_by_key(|info| info.last_access)
                .map(|info| info.last_access),
            newest_file: state.files.values()
                .max_by_key(|info| info.last_access)
                .map(|info| info.last_access),
        };
        debug!("Storage metrics: {:?}", metrics);
        Ok(metrics)
    }

    pub async fn get_file_stats(&self, key: &str) -> Result<FileStats, PluginError> {
        let state = self.state.read().await;
        if let Some(info) = state.files.get(key) {
            let stats = FileStats {
                size: info.size,
                last_access: info.last_access,
                path: info.path.clone(),
            };
            debug!("File stats for {}: {:?}", key, stats);
            Ok(stats)
        } else {
            warn!("File not found: {}", key);
            Err(PluginError::Storage("File not found".into()))
        }
    }
}

#[derive(Debug)]
pub struct StorageMetrics {
    pub total_space: u64,
    pub used_space: u64,
    pub file_count: usize,
    pub oldest_file: Option<Instant>,
    pub newest_file: Option<Instant>,
}

#[derive(Debug)]
pub struct FileStats {
    pub size: u64,
    pub last_access: Instant,
    pub path: PathBuf,
}

#[async_trait]
impl Plugin for StoragePlugin {
    fn name(&self) -> &str {
        "storage"
    }

    fn version(&self) -> &str {
        "1.0.0"
    }

    async fn init(&self) -> Result<(), PluginError> {
        info!("Initializing storage plugin");
        match tokio::fs::create_dir_all(&self.root_path).await {
            Ok(_) => {
                info!("Storage directory created: {:?}", self.root_path);
                Ok(())
            }
            Err(e) => {
                error!("Failed to create storage directory: {}", e);
                Err(PluginError::Storage(e.to_string()))
            }
        }
    }

    async fn cleanup(&self) -> Result<(), PluginError> {
        info!("Starting storage plugin cleanup");
        self.cleanup_expired_files().await?;
        info!("Storage plugin cleanup completed");
        Ok(())
    }

    async fn health_check(&self) -> Result<bool, PluginError> {
        let state = self.state.read().await;
        let healthy = state.used_space < state.max_space;
        let usage_percent = (state.used_space as f64 / state.max_space as f64) * 100.0;
        
        if healthy {
            info!("Storage health check passed. Usage: {:.1}%", usage_percent);
        } else {
            warn!("Storage health check failed. Usage: {:.1}%", usage_percent);
        }
        
        Ok(healthy)
    }
}

impl Clone for StoragePlugin {
    fn clone(&self) -> Self {
        Self {
            root_path: self.root_path.clone(),
            state: RwLock::new(StorageState {
                used_space: 0,
                max_space: 0,
                files: HashMap::new(),
            }),
        }
    }
} 