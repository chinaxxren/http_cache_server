use std::path::PathBuf;
use std::time::{Duration, Instant};
use super::metadata::CacheMetadata;

#[derive(Debug)]
pub(crate) struct CacheEntry {
    pub path: PathBuf,
    pub size: u64,
    pub last_access: Instant,
    pub metadata: CacheMetadata,
}

impl CacheEntry {
    pub fn is_expired(&self, ttl: Duration) -> bool {
        Instant::now().duration_since(self.last_access) > ttl
    }

    pub fn touch(&mut self) {
        self.last_access = Instant::now();
    }
}