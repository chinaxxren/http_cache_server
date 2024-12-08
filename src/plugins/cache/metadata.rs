use chrono::{DateTime, Utc};

#[derive(Debug, Clone)]
pub struct CacheMetadata {
    pub content_type: String,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl CacheMetadata {
    pub fn new(content_type: String) -> Self {
        Self {
            content_type,
            etag: None,
            last_modified: None,
            created_at: Utc::now(),
        }
    }

    pub fn with_etag(mut self, etag: String) -> Self {
        self.etag = Some(etag);
        self
    }

    pub fn with_last_modified(mut self, last_modified: String) -> Self {
        self.last_modified = Some(last_modified);
        self
    }
}