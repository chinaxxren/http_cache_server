use std::sync::Arc;
use std::net::SocketAddr;
use std::time::Duration;

use tracing::{info, warn, error};
use hyper::{Body, Client, Request};

use http_cache_server::{
    plugins::cache::CacheConfig,
    plugins::hls::HLSPlugin,
    prelude::*,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter("debug")
        .with_file(true)
        .with_line_number(true)
        .init();

    info!("Starting HLS proxy example");

    let cache = Arc::new(CacheManager::new(
        "./cache",
        CacheConfig::default(),
    ));

    let hls_plugin = Arc::new(HLSPlugin::new(
        "./cache/hls".to_string(),
        cache.clone(),
    ));

    let addr: SocketAddr = "127.0.0.1:3000".parse()?;
    let mut proxy_server = ProxyServer::new(addr, cache.clone());
    proxy_server.add_handler(hls_plugin);
    let proxy_server = Arc::new(proxy_server);

    // 启动代理服务器
    let server_handle = tokio::spawn({
        let proxy = proxy_server.clone();
        async move {
            if let Err(e) = proxy.run().await {
                error!("Server error: {}", e);
            }
        }
    });

    // 等待服务器启动
    tokio::time::sleep(Duration::from_secs(1)).await;

    // 测试 HLS 流
    let test_urls = vec![
        "http://example.com/stream.m3u8",
        "http://example.com/segment1.ts",
        "http://example.com/segment2.ts",
    ];

    let client = Client::new();

    for original_url in test_urls {
        info!("Testing proxy with URL: {}", original_url);

        let proxy_url = proxy_server.get_proxy_url(original_url)?;
        info!("Proxy URL: {}", proxy_url);

        let req = Request::builder()
            .uri(proxy_url)
            .body(Body::empty())?;

        match client.request(req).await {
            Ok(response) => {
                let status = response.status();
                info!("Response status: {}", status);

                if status.is_success() {
                    let body_bytes = hyper::body::to_bytes(response.into_body()).await?;
                    info!(
                        "Successfully received {} bytes through proxy",
                        body_bytes.len()
                    );
                }
            }
            Err(e) => warn!("Request failed: {}", e),
        }

        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    info!("HLS proxy test completed");
    Ok(())
}