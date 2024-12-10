use chrono::{DateTime, Utc};
use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheMetadata {
    pub content_type: String,
    pub last_modified: Option<String>,
    pub etag: Option<String>,
    pub ttl: Option<std::time::Duration>,
}

impl CacheMetadata {
    pub fn new(content_type: String) -> Self {
        Self {
            content_type,
            last_modified: None,
            etag: None,
            ttl: None,
        }
    }

    pub fn with_ttl(mut self, ttl: std::time::Duration) -> Self {
        self.ttl = Some(ttl);
        self
    }
}