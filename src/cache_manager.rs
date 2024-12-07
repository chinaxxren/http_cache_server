use std::sync::Arc;
use std::collections::HashMap;
use tokio::sync::RwLock;
use std::time::{Duration, SystemTime};
use crate::Result;
use crate::storage::Storage;

pub struct CacheManager {
    storage: Arc<Storage>,
    metadata: Arc<RwLock<HashMap<String, CacheEntry>>>,
    max_size: u64,
    max_age: Duration,
}

struct CacheEntry {
    size: u64,
    last_access: SystemTime,
    content_type: String,
}

impl CacheManager {
    pub fn new(storage: Arc<Storage>, max_size: u64, max_age: Duration) -> Self {
        Self {
            storage,
            metadata: Arc::new(RwLock::new(HashMap::new())),
            max_size,
            max_age,
        }
    }

    pub async fn store(&self, key: &str, data: Vec<u8>, content_type: String) -> Result<()> {
        let size = data.len() as u64;
        let entry = CacheEntry {
            size,
            last_access: SystemTime::now(),
            content_type,
        };

        // 检查并清理空间
        self.ensure_space(size).await?;

        // 存储数据
        self.storage.store_chunk(key, 0, &data).await?;
        self.metadata.write().await.insert(key.to_string(), entry);

        Ok(())
    }

    pub async fn get(&self, key: &str) -> Result<Option<(Vec<u8>, String)>> {
        if let Some(mut entry) = self.metadata.write().await.get_mut(key) {
            entry.last_access = SystemTime::now();
            if let Some(data) = self.storage.read_chunk(key, 0).await? {
                return Ok(Some((data, entry.content_type.clone())));
            }
        }
        Ok(None)
    }

    async fn ensure_space(&self, required_size: u64) -> Result<()> {
        let mut metadata = self.metadata.write().await;
        let mut current_size: u64 = metadata.values().map(|e| e.size).sum();

        if current_size + required_size > self.max_size {
            let now = SystemTime::now();
            let mut to_remove = Vec::new();

            // 先收集需要删除的键
            for (key, entry) in metadata.iter() {
                if current_size + required_size <= self.max_size {
                    break;
                }
                if now.duration_since(entry.last_access).unwrap() > self.max_age {
                    to_remove.push(key.clone());
                    current_size -= entry.size;
                }
            }

            // 然后删除收集到的键
            for key in to_remove {
                self.storage.remove(&key).await?;
                metadata.remove(&key);
            }
        }

        Ok(())
    }
} 