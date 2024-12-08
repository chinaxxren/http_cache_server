pub mod hls;
pub mod cache;

#[cfg(debug_assertions)]
mod init {
    use super::*;
    use std::sync::Once;
    use tracing::debug;
    
    static INIT: Once = Once::new();
    
    pub(crate) fn log_module_init() {
        INIT.call_once(|| {
            debug!("Initializing plugins module");
            debug!("Available plugins: HLS, Cache");
        });
    }
}

#[cfg(debug_assertions)]
pub(crate) use init::log_module_init;