use std::sync::Arc;
use std::net::SocketAddr;
use tracing::{info, error};
use http_cache_server::{
    plugins::cache::CacheConfig,
    plugins::hls::HLSPlugin,
    plugins::mp4::MP4Plugin,
    prelude::*,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter("debug")
        .init();
    
    info!("Starting proxy server");

    let cache = Arc::new(CacheManager::new(
        "./cache",
        CacheConfig::default(),
    ));

    let addr: SocketAddr = "127.0.0.1:3000".parse()?;
    let mut proxy_server = ProxyServer::new(addr, cache.clone());

    // 添加 HLS 插件
    let hls_plugin = Arc::new(HLSPlugin::new(cache.clone()));
    proxy_server.add_handler(hls_plugin);

    // 添加 MP4 插件
    let mp4_plugin = Arc::new(MP4Plugin::new(cache.clone()));
    proxy_server.add_handler(mp4_plugin);

    let proxy_server = Arc::new(proxy_server);

    if let Err(e) = proxy_server.run().await {
        error!("Server error: {}", e);
        return Err(e.into());
    }

    Ok(())
} 