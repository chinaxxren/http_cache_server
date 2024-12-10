use std::path::PathBuf;
use tokio::fs;
use crate::error::PluginError;

#[derive(Debug)]
pub struct StorageManager {
    base_path: PathBuf,
}

impl StorageManager {
    pub fn new(base_path: &str) -> Self {
        Self {
            base_path: PathBuf::from(base_path),
        }
    }

    pub async fn init(&self) -> Result<(), PluginError> {
        fs::create_dir_all(&self.base_path).await?;
        Ok(())
    }

    pub async fn cleanup(&self) -> Result<(), PluginError> {
        // 清理过期文件
        Ok(())
    }
} 