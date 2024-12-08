use std::path::PathBuf;
use std::time::Instant;
use super::metadata::CacheMetadata;

#[derive(Debug)]
pub struct CacheEntry {
    pub(crate) path: PathBuf,
    pub(crate) size: u64,
    pub(crate) last_access: Instant,
    pub(crate) metadata: CacheMetadata,
}

impl CacheEntry {
    pub fn new(path: PathBuf, size: u64, metadata: CacheMetadata) -> Self {
        Self {
            path,
            size,
            last_access: Instant::now(),
            metadata,
        }
    }

    pub fn touch(&mut self) {
        self.last_access = Instant::now();
    }

    pub fn is_expired(&self, ttl: std::time::Duration) -> bool {
        self.last_access.elapsed() > ttl
    }
}