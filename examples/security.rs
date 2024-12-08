use http_cache_server::prelude::*;
use std::sync::Arc;
use tracing::{info, warn, debug, instrument};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter("debug")
        .with_file(true)
        .with_line_number(true)
        .init();

    info!("Starting security example");

    let security_plugin = Arc::new(SecurityPlugin::new(100));
    debug!("Security plugin initialized with rate limit: 100");

    // 配置允许的来源
    let allowed_origins = vec![
        "https://example.com",
        "https://api.example.com",
    ];

    for origin in allowed_origins {
        debug!("Adding allowed origin: {}", origin);
        security_plugin.add_allowed_origin(origin.to_string()).await;
    }

    // 模拟请求检查
    let test_origins = vec![
        "https://example.com",
        "https://malicious.com",
        "https://api.example.com",
    ];

    info!("Starting origin validation tests");
    for origin in test_origins {
        debug!("Testing origin: {}", origin);
        match security_plugin.check_origin(origin).await {
            Ok(true) => info!("Origin allowed: {}", origin),
            Ok(false) => warn!("Origin blocked: {}", origin),
            Err(e) => warn!("Error checking origin {}: {}", origin, e),
        }
    }

    // 测试速率限制
    let client_id = "test_client";
    info!("Testing rate limit for client: {}", client_id);
    match security_plugin.check_rate_limit(client_id).await {
        Ok(true) => info!("Rate limit check passed for client: {}", client_id),
        Ok(false) => warn!("Rate limit exceeded for client: {}", client_id),
        Err(e) => warn!("Rate limit check error for client {}: {}", client_id, e),
    }

    info!("Security example completed");
    Ok(())
} 