use thiserror::Error;

#[derive(Debug, Error)]
pub enum CacheError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Cache full: needed {needed} bytes, but only {available} bytes available")]
    Full {
        needed: u64,
        available: u64,
    },

    #[error("Entry not found: {0}")]
    NotFound(String),

    #[error("Invalid path: {0}")]
    InvalidPath(String),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Cache error: {0}")]
    Other(String),
} 