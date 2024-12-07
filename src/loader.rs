use std::sync::Arc;
use tokio::sync::RwLock;
use crate::{Result, cache::Cache, network::NetworkClient, storage::Storage};
use crate::error::CacheError;

pub struct Loader {
    cache: Arc<Cache>,
    network: Arc<NetworkClient>,
    storage: Arc<Storage>,
    active_downloads: Arc<RwLock<Vec<String>>>,
}

impl Loader {
    pub fn new(cache: Arc<Cache>, network: Arc<NetworkClient>, storage: Arc<Storage>) -> Self {
        Self {
            cache,
            network,
            storage,
            active_downloads: Arc::new(RwLock::new(Vec::new())),
        }
    }

    pub async fn load(&self, url: &str) -> Result<Vec<u8>> {
        // 先检查缓存
        if let Some(data) = self.cache.get(url).await {
            return Ok(data);
        }

        // 检查是否正在下载
        {
            let active = self.active_downloads.read().await;
            if active.contains(&url.to_string()) {
                return Err(CacheError::Cache("Resource is being downloaded".to_string()));
            }
        }

        // 添加到活动下载列表
        {
            let mut active = self.active_downloads.write().await;
            active.push(url.to_string());
        }

        // 下载资源
        let result = self.network.get(url).await;

        // 从活动下载列表中移除
        {
            let mut active = self.active_downloads.write().await;
            if let Some(pos) = active.iter().position(|x| x == url) {
                active.remove(pos);
            }
        }

        // 处理下载结果
        let data = result?;
        self.cache.set(url.to_string(), data.clone()).await?;
        
        Ok(data)
    }
} 