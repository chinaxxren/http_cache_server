use std::time::Duration;

#[derive(Debug, Clone)]
pub struct CachePolicy {
    pub ttl: Duration,
    pub max_size: u64,
    pub min_free_space: u64,
}

impl Default for CachePolicy {
    fn default() -> Self {
        Self {
            ttl: Duration::from_secs(86400), // 24 hours
            max_size: 1024 * 1024 * 1024,    // 1GB
            min_free_space: 1024 * 1024 * 100, // 100MB
        }
    }
}

impl CachePolicy {
    pub fn with_ttl(mut self, ttl: Duration) -> Self {
        self.ttl = ttl;
        self
    }

    pub fn with_max_size(mut self, max_size: u64) -> Self {
        self.max_size = max_size;
        self
    }

    pub fn with_min_free_space(mut self, min_free_space: u64) -> Self {
        self.min_free_space = min_free_space;
        self
    }
} 