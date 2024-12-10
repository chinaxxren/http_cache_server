// use std::fmt;
use thiserror::Error;
use crate::plugins::cache::CacheError;

#[derive(Debug, Error)]
pub enum PluginError {
    #[error("Network error: {0}")]
    Network(String),

    #[error("Storage error: {0}")]
    Storage(String),

    #[error("HLS error: {0}")]
    Hls(String),

    #[error("MP4 error: {0}")]
    Mp4(String),

    #[error("Cache error: {0}")]
    Cache(#[from] CacheError),

    #[error("Other error: {0}")]
    Other(String),
}

impl From<std::io::Error> for PluginError {
    fn from(err: std::io::Error) -> Self {
        PluginError::Storage(err.to_string())
    }
} 