pub mod error;
pub mod plugin;
pub mod proxy;
pub mod utils;
pub mod plugins;

pub mod prelude {
    pub use crate::plugin::Plugin;
    pub use crate::plugins::hls::HLSPlugin;
    pub use crate::plugins::mp4::MP4Plugin;
    pub use crate::plugins::cache::{CacheManager, CacheConfig};
    pub use crate::proxy::{ProxyServer, MediaHandler};
} 
