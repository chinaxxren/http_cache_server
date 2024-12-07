use thiserror::Error;

#[derive(Error, Debug)]
pub enum CacheError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Network error: {0}")]
    Network(String),
    #[error("Cache error: {0}")]
    Cache(String),
    #[error("Invalid input: {0}")]
    InvalidInput(String),
    #[error("Configuration error: {0}")]
    Config(String),
    #[error("Performance error: {0}")]
    Performance(String),
    #[error("Resource limit exceeded: {0}")]
    ResourceLimit(String),
    #[error("Timeout error: {0}")]
    Timeout(String),
} 