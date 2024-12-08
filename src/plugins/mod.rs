pub mod hls;
pub mod storage;
pub mod security;
pub mod cache;
pub mod bandwidth;
pub mod network;

use tracing::debug;

// 在模块初始化时记录日志
#[cfg(debug_assertions)]
mod init {
    use super::*;
    use std::sync::Once;
    
    static INIT: Once = Once::new();
    
    pub(crate) fn log_module_init() {
        INIT.call_once(|| {
            debug!("Initializing plugins module");
            debug!("Available plugins:");
            debug!(" - HLS Plugin");
            debug!(" - Storage Plugin");
            debug!(" - Security Plugin");
            debug!(" - Network Manager");
            debug!(" - Bandwidth Manager");
        });
    }
}

#[cfg(debug_assertions)]
pub(crate) use init::log_module_init;

// Re-export managers for internal use
pub(crate) use network::NetworkManager;
pub(crate) use bandwidth::BandwidthManager; 