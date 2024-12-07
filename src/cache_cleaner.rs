use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::time;
use dashmap::DashMap;
use crate::Result;
use crate::storage::Storage;

pub struct CacheCleaner {
    storage: Arc<Storage>,
    cache_ttl: Duration,
    max_size: u64,
    access_times: Arc<DashMap<String, SystemTime>>,
}

impl CacheCleaner {
    pub fn new(storage: Arc<Storage>, cache_ttl: Duration, max_size: u64) -> Self {
        Self {
            storage,
            cache_ttl,
            max_size,
            access_times: Arc::new(DashMap::new()),
        }
    }

    pub fn record_access(&self, key: &str) {
        self.access_times.insert(key.to_string(), SystemTime::now());
    }

    pub async fn start_cleaning_task(self: Arc<Self>) {
        let cleaner = self.clone();
        tokio::spawn(async move {
            let mut interval = time::interval(Duration::from_secs(3600)); // 每小时清理一次
            loop {
                interval.tick().await;
                if let Err(e) = cleaner.clean_expired_cache().await {
                    eprintln!("Cache cleaning error: {}", e);
                }
            }
        });
    }

    async fn clean_expired_cache(&self) -> Result<()> {
        let now = SystemTime::now();
        let mut to_remove = Vec::new();
        let mut total_size: u64 = 0;

        // 找出过期的缓存和计算总大小
        for entry in self.access_times.iter() {
            if let Ok(elapsed) = entry.value().elapsed() {
                if elapsed > self.cache_ttl {
                    to_remove.push(entry.key().clone());
                } else {
                    if let Ok(size) = self.storage.get_size(entry.key()).await {
                        total_size += size;
                    }
                }
            }
        }

        // 如果总大小超过限制，添加最旧的缓存到删除列表
        if total_size > self.max_size {
            let mut entries: Vec<_> = self.access_times.iter().collect();
            entries.sort_by_key(|entry| *entry.value());
            
            let mut current_size = total_size;
            for entry in entries {
                if current_size <= self.max_size {
                    break;
                }
                if let Ok(size) = self.storage.get_size(entry.key()).await {
                    current_size -= size;
                    to_remove.push(entry.key().clone());
                }
            }
        }

        // 删除缓存
        for key in to_remove {
            self.access_times.remove(&key);
            self.storage.remove(&key).await?;
        }

        Ok(())
    }
} 