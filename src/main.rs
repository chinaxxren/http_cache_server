use std::sync::Arc;
use std::net::SocketAddr;
use tracing::{info, error};
use http_cache_server::{
    plugins::cache::CacheConfig,
    prelude::*,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .init();
    
    info!("Starting HTTP Cache Server");
    
    let cache = Arc::new(CacheManager::new(
        "./cache",
        CacheConfig::default(),
    ));

    let hls_plugin = Arc::new(HLSPlugin::new(
        "./cache/hls".to_string(),
        cache.clone(),
    ));

    let addr: SocketAddr = "127.0.0.1:3000".parse()?;
    let proxy_server = Arc::new(ProxyServer::new(addr, hls_plugin, cache));

    if let Err(e) = proxy_server.run().await {
        error!("Server error: {}", e);
        return Err(e.into());
    }

    Ok(())
} 