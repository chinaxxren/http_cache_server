use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::RwLock;
use crate::Result;
use crate::storage::Storage;

pub struct CacheStrategy {
    storage: Arc<Storage>,
    access_times: Arc<RwLock<HashMap<String, SystemTime>>>,
    access_counts: Arc<RwLock<HashMap<String, u64>>>,
    max_age: Duration,
    max_size: u64,
    min_access_count: u64,
    current_size: Arc<RwLock<u64>>,
}

impl CacheStrategy {
    pub fn new(
        storage: Arc<Storage>,
        max_age: Duration,
        max_size: u64,
        min_access_count: u64,
    ) -> Self {
        Self {
            storage,
            access_times: Arc::new(RwLock::new(HashMap::new())),
            access_counts: Arc::new(RwLock::new(HashMap::new())),
            max_age,
            max_size,
            min_access_count,
            current_size: Arc::new(RwLock::new(0)),
        }
    }

    // 记录访问
    pub async fn record_access(&self, key: &str, size: u64) {
        let mut access_times = self.access_times.write().await;
        let mut access_counts = self.access_counts.write().await;
        let mut current_size = self.current_size.write().await;

        access_times.insert(key.to_string(), SystemTime::now());
        *access_counts.entry(key.to_string()).or_insert(0) += 1;
        *current_size += size;
    }

    // 基于时间的清理
    async fn clean_by_time(&self) -> Result<HashSet<String>> {
        let now = SystemTime::now();
        let mut to_remove = HashSet::new();
        let access_times = self.access_times.read().await;

        for (key, &last_access) in access_times.iter() {
            if now.duration_since(last_access).unwrap() > self.max_age {
                to_remove.insert(key.clone());
            }
        }

        Ok(to_remove)
    }

    // 基于容量的 LRU 清理
    async fn clean_by_size(&self) -> Result<HashSet<String>> {
        let mut to_remove = HashSet::new();
        let current_size = *self.current_size.read().await;

        if current_size <= self.max_size {
            return Ok(to_remove);
        }

        let access_times = self.access_times.read().await;
        let mut entries: Vec<_> = access_times.iter().collect();
        entries.sort_by_key(|(_, &time)| time);

        let mut freed_size = 0u64;
        let target_size = current_size - self.max_size;

        for (key, _) in entries {
            if let Ok(size) = self.storage.get_size(key).await {
                freed_size += size;
                to_remove.insert(key.clone());
                if freed_size >= target_size {
                    break;
                }
            }
        }

        Ok(to_remove)
    }

    // 基于访问频率的清理
    async fn clean_by_frequency(&self) -> Result<HashSet<String>> {
        let mut to_remove = HashSet::new();
        let access_counts = self.access_counts.read().await;

        for (key, &count) in access_counts.iter() {
            if count < self.min_access_count {
                to_remove.insert(key.clone());
            }
        }

        Ok(to_remove)
    }

    // 执行清理
    pub async fn clean(&self) -> Result<()> {
        // 合并所有清理策略的结果
        let mut to_remove = HashSet::new();
        to_remove.extend(self.clean_by_time().await?);
        to_remove.extend(self.clean_by_size().await?);
        to_remove.extend(self.clean_by_frequency().await?);

        // 执行清理
        let mut current_size = self.current_size.write().await;
        let mut access_times = self.access_times.write().await;
        let mut access_counts = self.access_counts.write().await;

        for key in to_remove {
            if let Ok(size) = self.storage.get_size(&key).await {
                self.storage.remove(&key).await?;
                *current_size -= size;
                access_times.remove(&key);
                access_counts.remove(&key);
            }
        }

        Ok(())
    }

    // 启动定期清理任务
    pub async fn start_cleaning_task(self: Arc<Self>) {
        let cleaner = self.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(3600));
            loop {
                interval.tick().await;
                if let Err(e) = cleaner.clean().await {
                    eprintln!("Cache cleaning error: {}", e);
                }
            }
        });
    }
} 