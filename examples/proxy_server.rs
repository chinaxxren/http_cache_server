use std::sync::Arc;
use std::net::SocketAddr;
use tokio::signal;
use tracing::{info, error};

use http_cache_server::{
    plugins::cache::{CacheManager, CacheConfig},
    plugins::hls::HLSPlugin,
    plugins::mp4::MP4Plugin,
    prelude::*,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 初始化日志
    tracing_subscriber::fmt()
        .with_env_filter("debug")
        .with_file(true)
        .with_line_number(true)
        .with_thread_ids(true)
        .with_thread_names(true)
        .init();
    
    info!("Starting proxy server");

    // 创建缓存管理器
    let cache_config = CacheConfig {
        max_space: 10 * 1024 * 1024 * 1024, // 10GB
        entry_ttl: std::time::Duration::from_secs(3600 * 24), // 24小时
        min_free_space: 1024 * 1024 * 100, // 100MB
    };
    let cache = Arc::new(CacheManager::new("cache", cache_config));

    // 创建插件
    let hls_plugin = Arc::new(HLSPlugin::new(cache.clone()));
    let mp4_plugin = Arc::new(MP4Plugin::new(cache.clone()));

    // 创建代理服务器
    let addr: SocketAddr = "127.0.0.1:8080".parse()?;
    info!("Creating proxy server on {}", addr);
    let mut proxy_server = ProxyServer::new(addr, cache.clone());

    // 添加处理器
    proxy_server.add_handler(hls_plugin);
    proxy_server.add_handler(mp4_plugin);

    // 启动服务器
    let server = Arc::new(proxy_server);
    let server_clone = server.clone();
    let server_handle = tokio::spawn(async move {
        info!("Starting server on {}", addr);
        if let Err(e) = server_clone.run().await {
            error!("Server error: {}", e);
        }
    });

    // 等待中断信号
    info!("Server is ready. Press Ctrl+C to stop...");
    match signal::ctrl_c().await {
        Ok(()) => info!("Shutdown signal received"),
        Err(e) => error!("Failed to listen for shutdown signal: {}", e),
    }

    // 优雅关闭
    info!("Shutting down server...");
    server_handle.abort();
    info!("Server shutdown completed");

    Ok(())
} 