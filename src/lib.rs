use serde::{Serialize, Deserialize};

pub mod error;
pub mod config;
pub mod plugin;
pub mod plugin_manager;
pub mod proxy;

// 插件模块
pub mod plugins;

// Re-export plugins
pub mod prelude {
    pub use crate::plugin::Plugin;
    pub use crate::plugins::hls::HLSPlugin;
    pub use crate::plugins::storage::StoragePlugin;
    pub use crate::plugins::security::SecurityPlugin;
    pub use crate::plugins::cache::CacheManager;
    pub use crate::plugins::bandwidth::BandwidthManager;
    pub use crate::config::Config;
    pub use crate::error::PluginError;
    pub use crate::proxy::ProxyServer;
} 