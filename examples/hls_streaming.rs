use http_cache_server::prelude::*;
use std::sync::Arc;
use tracing::{info, warn, error};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().with_env_filter("info").init();

    let hls_plugin = Arc::new(HLSPlugin::new("./cache".to_string()));

    // 添加多个码率的流
    let stream_id = "live_stream";
    hls_plugin.add_stream(stream_id.to_string(), 5000.0).await?;

    // 模拟下载不同码率的分片
    let segments = vec![
        "http://example.com/high/segment1.ts",
        "http://example.com/medium/segment1.ts",
        "http://example.com/low/segment1.ts",
    ];

    for (i, url) in segments.iter().enumerate() {
        info!("Downloading segment at quality level {}", i);
        match hls_plugin.download_segment(url, stream_id).await {
            Ok(data) => {
                info!("Successfully downloaded {} bytes", data.len());
                // 模拟带宽变化
                let new_bandwidth = 5000.0 - (i as f64 * 1000.0);
                hls_plugin.update_bandwidth(stream_id, new_bandwidth).await?;
            }
            Err(e) => error!("Failed to download segment: {}", e),
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    Ok(())
} 