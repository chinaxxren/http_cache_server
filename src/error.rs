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
    #[error("Hyper error: {0}")]
    Hyper(#[from] hyper::Error),
} 