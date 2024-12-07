use std::sync::Arc;
use tokio::sync::Semaphore;
use crate::Result;

pub struct ResourceManager {
    concurrent_downloads: Arc<Semaphore>,
}

impl ResourceManager {
    pub fn new(max_concurrent: usize) -> Self {
        Self {
            concurrent_downloads: Arc::new(Semaphore::new(max_concurrent)),
        }
    }

    pub async fn acquire(&self) -> Result<ResourceGuard<'_>> {
        let permit = self.concurrent_downloads.acquire().await
            .map_err(|e| crate::error::CacheError::Cache(e.to_string()))?;
        Ok(ResourceGuard { _permit: permit })
    }
}

pub struct ResourceGuard<'a> {
    _permit: tokio::sync::SemaphorePermit<'a>,
} 