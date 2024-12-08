use std::sync::Arc;
use std::net::SocketAddr;
use std::time::Duration;

use tracing::{info, warn, error};
use hyper;

use http_cache_server::{
    plugins::cache::{CacheConfig, CacheManager, CacheMetadata},
    prelude::*,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 初始化日志
    tracing_subscriber::fmt()
        .with_env_filter("debug")
        .with_file(true)
        .with_line_number(true)
        .init();

    info!("Starting HLS proxy example with caching");

    // 创建缓存管理器
    let cache = Arc::new(CacheManager::new(
        "./cache",
        CacheConfig::default(),
    ));

    // 创建 HLS 插件
    let hls_plugin = Arc::new(HLSPlugin::new(
        "./cache/hls".to_string(),
        cache.clone(),
    ));

    // 创建代理服务器
    let addr: SocketAddr = "127.0.0.1:0".parse()?;  // 使用随机端口
    let proxy = Arc::new(ProxyServer::new(addr, hls_plugin.clone(), cache.clone()));

    // 启动代理服务器
    let server_handle = tokio::spawn({
        let proxy = proxy.clone();
        async move {
            if let Err(e) = proxy.run().await {
                error!("Server error: {}", e);
            }
        }
    });

    // 原始 HLS 流 URL
    let original_url = "http://example.com/live/stream.m3u8";
    
    // 创建代理 URL
    let proxy_url = match proxy.get_proxy_url(original_url) {
        Ok(url) => {
            info!("Original URL: {}", original_url);
            info!("Proxy URL: {}", url);
            url
        }
        Err(e) => {
            error!("Failed to create proxy URL: {}", e);
            return Err(e.into());
        }
    };

    // 等待服务器启动
    tokio::time::sleep(Duration::from_secs(1)).await;

    // 创建 HTTP 客户端
    let client = hyper::Client::new();

    // 模拟客户端请求代理 URL
    info!("Sending request to proxy...");
    match client.get(proxy_url.parse()?).await {
        Ok(response) => {
            let status = response.status();
            info!("Proxy response status: {}", status);

            if status.is_success() {
                // 读取响应体
                let body_bytes = hyper::body::to_bytes(response.into_body()).await?;
                info!(
                    "Successfully received {} bytes through proxy",
                    body_bytes.len()
                );
            }
        }
        Err(e) => warn!("Request failed: {}", e),
    }

    Ok(())
}