use http_cache_server::prelude::*;
use std::sync::Arc;
use tracing::{info, warn, error, debug, instrument};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter("debug")
        .with_file(true)
        .with_line_number(true)
        .init();

    info!("Starting HLS streaming example");

    // 初始化 HLS 插件
    let hls_plugin = Arc::new(HLSPlugin::new("./cache".to_string()));
    debug!("HLS plugin initialized");

    // 创建测试流
    let stream_id = "live_stream";
    let initial_bandwidth = 5000.0;
    info!("Creating stream {} with initial bandwidth {}", stream_id, initial_bandwidth);
    hls_plugin.add_stream(stream_id.to_string(), initial_bandwidth).await?;

    // 检查初始状态
    if let Ok(metrics) = hls_plugin.get_stream_metrics().await {
        info!("Initial stream metrics:");
        info!("  - Active streams: {}", metrics.active_streams);
        info!("  - Total segments: {}", metrics.total_segments);
        info!("  - Average bandwidth: {:.2} bps", metrics.avg_bandwidth);
    }

    // 模拟不同质量级别的分片下载
    let quality_levels = vec![
        ("high", "http://example.com/high/segment1.ts", 6000.0),
        ("medium", "http://example.com/medium/segment1.ts", 4000.0),
        ("low", "http://example.com/low/segment1.ts", 2000.0),
    ];

    info!("Starting adaptive streaming simulation");
    for (quality, url, bandwidth) in quality_levels {
        debug!("Testing {} quality level", quality);
        
        // 更新带宽
        info!("Updating bandwidth to {} bps", bandwidth);
        hls_plugin.update_bandwidth(stream_id, bandwidth).await?;

        // 获取流信息
        if let Some(stream_info) = hls_plugin.get_stream_info(stream_id).await {
            debug!(
                "Stream status: bandwidth={}, segments={}", 
                stream_info.bandwidth,
                stream_info.segments.len()
            );
        }

        // 下载分片
        info!("Downloading segment: {}", url);
        match hls_plugin.download_segment(url, stream_id).await {
            Ok(data) => {
                let size = data.len();
                info!(
                    "Successfully downloaded {} quality segment: {} bytes",
                    quality, size
                );
                
                // 计算实际吞吐量
                let throughput = (size as f64 * 8.0) / 1000.0; // Kbps
                debug!(
                    "Effective throughput for {} quality: {:.2} Kbps",
                    quality, throughput
                );
            }
            Err(e) => {
                error!("Failed to download {} quality segment: {}", quality, e);
                warn!("Falling back to lower quality");
            }
        }

        debug!("Waiting before next quality test");
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    // 清理资源
    info!("Cleaning up resources");
    let active_streams = hls_plugin.get_active_stream_count().await;
    info!("Active streams before cleanup: {}", active_streams);
    
    match hls_plugin.cleanup_old_streams().await {
        Ok(_) => {
            info!("Stream cleanup completed successfully");
            // 检查最终状态
            if let Ok(metrics) = hls_plugin.get_stream_metrics().await {
                info!("Final stream metrics:");
                info!("  - Active streams: {}", metrics.active_streams);
                info!("  - Total segments: {}", metrics.total_segments);
                info!("  - Average bandwidth: {:.2} bps", metrics.avg_bandwidth);
            }
        }
        Err(e) => error!("Error during stream cleanup: {}", e),
    }

    info!("HLS streaming example completed");
    Ok(())
} 