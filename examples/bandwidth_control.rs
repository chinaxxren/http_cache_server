use http_cache_server::prelude::*;
use std::sync::Arc;
use tracing::{info, warn, debug, instrument};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter("debug")
        .with_file(true)
        .with_line_number(true)
        .init();

    info!("Starting bandwidth control example");

    let hls_plugin = Arc::new(HLSPlugin::new("./cache".to_string()));
    debug!("HLS plugin initialized");
    
    // 设置不同的带宽限制
    let stream_id = "test_stream";
    let bandwidths = vec![6000.0, 4000.0, 2000.0]; // 不同的带宽级别

    for bandwidth in bandwidths {
        info!("Testing bandwidth limit: {} bps", bandwidth);
        
        // 添加新的流并设置带宽限制
        let current_stream = format!("{}_{}", stream_id, bandwidth);
        debug!("Creating stream {} with bandwidth {}", current_stream, bandwidth);
        hls_plugin.add_stream(current_stream.clone(), bandwidth).await?;
        
        // 尝试下载大文件
        let url = "http://example.com/large_file.ts";
        info!("Attempting to download: {}", url);
        match hls_plugin.download_segment(url, &current_stream).await {
            Ok(data) => {
                let throughput = data.len() as f64 / 1024.0; // KB
                info!("Downloaded {:.2} KB at {:.2} Kbps", throughput, bandwidth/1024.0);
            }
            Err(e) => warn!("Download limited by bandwidth: {}", e),
        }

        debug!("Waiting before next test");
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    info!("Bandwidth control example completed");
    Ok(())
} 