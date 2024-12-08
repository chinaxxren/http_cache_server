use crate::plugin::Plugin;
use crate::error::PluginError;
use async_trait::async_trait;
use std::path::PathBuf;
use tokio::sync::RwLock;
use std::collections::HashMap;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct StoragePlugin {
    root_path: PathBuf,
    state: RwLock<StorageState>,
}

#[derive(Debug)]
struct StorageState {
    used_space: u64,
    max_space: u64,
    files: HashMap<String, FileInfo>,
}

#[derive(Debug)]
struct FileInfo {
    path: PathBuf,
    size: u64,
    last_access: Instant,
}

impl StoragePlugin {
    pub fn new(root_path: PathBuf, max_space: u64) -> Self {
        Self {
            root_path,
            state: RwLock::new(StorageState {
                used_space: 0,
                max_space,
                files: HashMap::new(),
            }),
        }
    }

    pub async fn store_file(&self, key: String, data: Vec<u8>) -> Result<(), PluginError> {
        let mut state = self.state.write().await;
        let file_size = data.len() as u64;

        // 检查空间
        if state.used_space + file_size > state.max_space {
            return Err(PluginError::Storage("Storage space exceeded".into()));
        }

        // 存储文件
        let file_path = self.root_path.join(&key);
        tokio::fs::write(&file_path, data).await?;

        // 更新状态
        state.files.insert(key, FileInfo {
            path: file_path,
            size: file_size,
            last_access: Instant::now(),
        });
        state.used_space += file_size;

        Ok(())
    }

    pub async fn get_file(&self, key: &str) -> Result<Vec<u8>, PluginError> {
        let mut state = self.state.write().await;
        if let Some(file_info) = state.files.get_mut(key) {
            file_info.last_access = Instant::now();
            Ok(tokio::fs::read(&file_info.path).await?)
        } else {
            Err(PluginError::Storage("File not found".into()))
        }
    }

    pub async fn cleanup_old_files(&self, max_age: Duration) -> Result<(), PluginError> {
        let mut state = self.state.write().await;
        let now = Instant::now();
        let mut to_remove = Vec::new();

        // 找出过期文件
        for (key, info) in state.files.iter() {
            if now.duration_since(info.last_access) > max_age {
                to_remove.push(key.clone());
            }
        }

        // 删除过期文件
        for key in to_remove {
            if let Some(info) = state.files.remove(&key) {
                tokio::fs::remove_file(&info.path).await?;
                state.used_space -= info.size;
            }
        }

        Ok(())
    }
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
        tokio::fs::create_dir_all(&self.root_path).await?;
        Ok(())
    }

    async fn cleanup(&self) -> Result<(), PluginError> {
        self.cleanup_old_files(Duration::from_secs(3600)).await?;
        Ok(())
    }

    async fn health_check(&self) -> Result<bool, PluginError> {
        let state = self.state.read().await;
        Ok(state.used_space < state.max_space)
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