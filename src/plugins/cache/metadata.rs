use chrono::{DateTime, Utc};
use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheMetadata {
    pub content_type: String,
    pub etag: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl CacheMetadata {
    pub fn new(content_type: String) -> Self {
        Self {
            content_type,
            etag: None,
            created_at: Utc::now(),
        }
    }

    pub fn with_etag(mut self, etag: String) -> Self {
        self.etag = Some(etag);
        self
    }

    pub fn content_type(&self) -> &str {
        &self.content_type
    }
}