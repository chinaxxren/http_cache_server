use std::sync::Arc;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::signal;
use tracing::{info, warn, error};
use hyper::{Body, Client, Request};

use http_cache_server::{
    plugins::cache::CacheConfig,
    plugins::mp4::MP4Plugin,
    prelude::*,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 初始化日志
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .init();
    
    info!("Starting MP4 proxy example");

    // 创建缓存管理器
    let cache = Arc::new(CacheManager::new(
        "./cache",
        CacheConfig::default(),
    ));

    // 创建并配置代理服务器
    let addr: SocketAddr = "127.0.0.1:3000".parse()?;
    let mut proxy_server = ProxyServer::new(addr, cache.clone());
    let mp4_plugin = Arc::new(MP4Plugin::new(cache.clone()));
    proxy_server.add_handler(mp4_plugin);
    let proxy_server = Arc::new(proxy_server);

    // 测试单个 MP4 文件
    let test_url = "http://commondatastorage.googleapis.com/gtv-videos-bucket/sample/BigBuckBunny.mp4";
    info!("Testing MP4 file: {}", test_url);

    let proxy_url = proxy_server.get_proxy_url(test_url)?;
    info!("Proxy URL: {}", proxy_url);

    // 启动服务器
    let server_task = {
        let server = proxy_server.clone();
        tokio::spawn(async move {
            info!("Server starting...");
            if let Err(e) = server.run().await {
                error!("Server error: {}", e);
            }
        })
    };

    // 等待服务器启动
    tokio::time::sleep(Duration::from_secs(1)).await;



    // 发送请求
    let client = Client::new();
    let req = Request::builder()
        .uri(proxy_url)
        .body(Body::empty())?;

    match client.request(req).await {
        Ok(response) => {
            let status = response.status();
            info!("Response status: {}", status);

            if status.is_success() {
                let body_bytes = hyper::body::to_bytes(response.into_body()).await?;
                info!("Successfully received {} bytes", body_bytes.len());

                // 验证 MP4 文件头
                if body_bytes.len() > 8 {
                    let signature = &body_bytes[4..8];
                    if signature == b"ftyp" {
                        info!("Valid MP4 file signature detected");
                    } else {
                        warn!("Invalid MP4 file signature");
                    }
                }
            }
        }
        Err(e) => error!("Request failed: {}", e),
    }

    // 等待中断信号
    match signal::ctrl_c().await {
        Ok(()) => info!("Shutdown signal received"),
        Err(e) => error!("Failed to listen for shutdown signal: {}", e),
    }

    // 关闭服务器
    server_task.abort();
    info!("Server shutdown completed");

    Ok(())
} 