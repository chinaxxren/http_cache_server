pub mod error {
    use thiserror::Error;
    use std::io;

    #[derive(Error, Debug)]
    pub enum PluginError {
        #[error("IO error: {0}")]
        Io(#[from] io::Error),
        
        #[error("Plugin error: {0}")]
        Plugin(String),
        
        #[error("Initialization error: {0}")]
        Init(String),
        
        #[error("Network error: {0}")]
        Network(String),
        
        #[error("Config error: {0}")]
        Config(String),
        
        #[error("HLS error: {0}")]
        Hls(String),
        
        #[error("Storage error: {0}")]
        Storage(String),
        
        #[error("Security error: {0}")]
        Security(String),
    }
}

pub type Result<T> = std::result::Result<T, error::PluginError>;

mod plugin;
mod plugin_manager;
mod plugins;
pub mod config;

pub use plugin::Plugin;
pub use plugin_manager::PluginManager;
pub use config::Config;

// Re-export plugins
pub mod prelude {
    pub use crate::plugin::Plugin;
    pub use crate::plugins::hls::HLSPlugin;
    pub use crate::plugins::storage::StoragePlugin;
    pub use crate::plugins::security::SecurityPlugin;
    pub use crate::config::Config;
    pub use crate::error::PluginError;
} 