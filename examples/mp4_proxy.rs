use std::sync::Arc;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::signal;
use tracing::{info, warn, error};
use hyper::{Body, Client, Request};

use http_cache_server::{
    plugins::cache::{CacheManager, CacheConfig},
    plugins::mp4::MP4Plugin,
    prelude::*,
};

// 测试视频列表
const TEST_FILES: &[(&str, &str)] = &[
    (
        "Big Buck Bunny (Small)", 
        "https://sample-videos.com/video321/mp4/240/big_buck_bunny_240p_1mb.mp4"
    ),
    // (
    //     "Sample Video",
    //     "https://sample-videos.com/video321/mp4/240/sample_240p_1mb.mp4"
    // ),
];

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
    
    info!("Starting MP4 proxy example");

    // 创建缓存管理器
    let cache_dir = "./cache";
    info!("Creating cache directory: {}", cache_dir);
    std::fs::create_dir_all(cache_dir)?;
    
    let cache_config = CacheConfig {
        max_space: 1024 * 1024 * 1024, // 1GB
        entry_ttl: Duration::from_secs(3600), // 1小时
        min_free_space: 1024 * 1024 * 100, // 100MB
    };
    let cache = Arc::new(CacheManager::new(cache_dir, cache_config));

    // 创建并配置代理服务器
    let addr: SocketAddr = "127.0.0.1:3001".parse()?;
    info!("Creating proxy server on {}", addr);
    let mut proxy_server = ProxyServer::new(addr, cache.clone());
    let mp4_plugin = Arc::new(MP4Plugin::new(cache.clone()));
    proxy_server.add_handler(mp4_plugin);
    let proxy_server = Arc::new(proxy_server);

    // 启动服务器
    let server_task = {
        let server = proxy_server.clone();
        tokio::spawn(async move {
            info!("Server starting on {}", addr);
            if let Err(e) = server.run().await {
                error!("Server error: {}", e);
            }
        })
    };

    // 等待服务器启动
    info!("Waiting for server to start...");
    tokio::time::sleep(Duration::from_secs(1)).await;

    // 测试所有视频
    for (name, url) in TEST_FILES {
        info!("=== Testing {} ===", name);
        info!("Original URL: {}", url);
        
        // 获取代理 URL
        info!("Getting proxy URL for {}", url);
        let proxy_url = proxy_server.get_proxy_url(url).await?;
        info!("Proxy URL: {}", proxy_url);

        // 创建客户端请求
        info!("Creating request for {}", name);
        let client = Client::new();
        let req = Request::builder()
            .uri(proxy_url.clone())
            .body(Body::empty())?;

        // 发送请求到代理服务器
        info!("Sending request to proxy for {}", name);
        match client.request(req).await {
            Ok(response) => {
                let status = response.status();
                info!("Response status for {}: {}", name, status);

                if status.is_success() {
                    let mut body_bytes = 0;
                    let mut body = response.into_body();
                    let start_time = std::time::Instant::now();
                    
                    while let Some(chunk) = hyper::body::HttpBody::data(&mut body).await {
                        match chunk {
                            Ok(data) => {
                                body_bytes += data.len();
                                if body_bytes % (1024 * 1024) == 0 {
                                    let elapsed = start_time.elapsed();
                                    let speed = body_bytes as f64 / (1024.0 * 1024.0 * elapsed.as_secs_f64());
                                    info!("Progress for {}: {} MB received, {:.2} MB/s", 
                                        name, 
                                        body_bytes / (1024 * 1024),
                                        speed
                                    );
                                }
                            }
                            Err(e) => {
                                error!("Error reading response for {}: {}", name, e);
                                break;
                            }
                        }
                    }
                    
                    let elapsed = start_time.elapsed();
                    let speed = body_bytes as f64 / (1024.0 * 1024.0 * elapsed.as_secs_f64());
                    info!("Download completed for {}:", name);
                    info!("  - Total size: {} bytes", body_bytes);
                    info!("  - Time taken: {:.2} seconds", elapsed.as_secs_f64());
                    info!("  - Average speed: {:.2} MB/s", speed);
                }
            }
            Err(e) => error!("Request failed for {}: {}", name, e),
        }

        info!("Waiting before next test...");
        tokio::time::sleep(Duration::from_secs(2)).await;
    }

    // 等待中断信号
    info!("Waiting for shutdown signal (Ctrl+C)...");
    match signal::ctrl_c().await {
        Ok(()) => info!("Shutdown signal received"),
        Err(e) => error!("Failed to listen for shutdown signal: {}", e),
    }

    // 关闭服务器
    info!("Shutting down server...");
    server_task.abort();
    info!("Server shutdown completed");

    Ok(())
} 