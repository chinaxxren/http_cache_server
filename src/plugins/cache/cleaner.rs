use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn, debug};
use super::CacheManager;

pub struct CacheCleaner {
    cache: Arc<CacheManager>,
    interval: Duration,
}

impl CacheCleaner {
    pub fn new(cache: Arc<CacheManager>, interval: Duration) -> Self {
        Self {
            cache,
            interval,
        }
    }

    pub async fn start(&self) {
        info!("Starting cache cleaner with interval {:?}", self.interval);
        let cache = self.cache.clone();
        let interval = self.interval;

        tokio::spawn(async move {
            loop {
                debug!("Running cache cleanup cycle");
                match cache.cleanup().await {
                    Ok(_) => debug!("Cache cleanup completed successfully"),
                    Err(e) => warn!("Cache cleanup failed: {}", e),
                }
                tokio::time::sleep(interval).await;
            }
        });
    }
} 