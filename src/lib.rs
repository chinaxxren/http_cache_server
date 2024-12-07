pub mod proxy;
pub mod cache;
pub mod loader;
pub mod storage;
pub mod url_mapper;
pub mod resource;
pub mod hls;
pub mod network;
pub mod bandwidth;
pub mod cache_cleaner;
pub mod bandwidth_control;
pub mod cache_manager;
pub mod cache_strategy;
pub mod performance;

pub mod error {
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
}

pub type Result<T> = std::result::Result<T, error::CacheError>; 