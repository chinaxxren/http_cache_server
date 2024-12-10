use thiserror::Error;

#[derive(Debug, Error)]
pub enum CacheError {
    #[error("Cache entry not found: {0}")]
    NotFound(String),

    #[error("IO error: {0}")]
    IO(String),

    #[error("Storage error: {0}")]
    Storage(String),

    #[error("Invalid path: {0}")]
    InvalidPath(String),

    #[error("Cache full: needed {needed} bytes, but only {available} bytes available")]
    Full {
        needed: u64,
        available: u64,
    },

    #[error("Cache error: {0}")]
    Other(String),
}

impl From<std::io::Error> for CacheError {
    fn from(err: std::io::Error) -> Self {
        CacheError::IO(err.to_string())
    }
}

impl From<serde_json::Error> for CacheError {
    fn from(err: serde_json::Error) -> Self {
        CacheError::Storage(err.to_string())
    }
} 