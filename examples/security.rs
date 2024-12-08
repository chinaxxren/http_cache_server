use http_cache_server::prelude::*;
use std::sync::Arc;
use tracing::info;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().with_env_filter("info").init();

    let security_plugin = Arc::new(SecurityPlugin::new(100));

    // 配置允许的来源
    let allowed_origins = vec![
        "https://example.com",
        "https://api.example.com",
    ];

    for origin in allowed_origins {
        security_plugin.add_allowed_origin(origin.to_string()).await;
        info!("Added allowed origin: {}", origin);
    }

    // 模拟请求检查
    let test_origins = vec![
        "https://example.com",
        "https://malicious.com",
        "https://api.example.com",
    ];

    for origin in test_origins {
        match security_plugin.check_origin(origin).await {
            Ok(true) => info!("Origin allowed: {}", origin),
            Ok(false) => info!("Origin blocked: {}", origin),
            Err(e) => info!("Error checking origin {}: {}", origin, e),
        }
    }

    Ok(())
} 