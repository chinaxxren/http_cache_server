use http_cache_server::prelude::*;
use http_cache_server::PluginManager;
use std::sync::Arc;
use tracing::{info, warn, error, debug, instrument};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 初始化日志
    tracing_subscriber::fmt()
        .with_env_filter("debug")
        .with_file(true)
        .with_line_number(true)
        .init();

    info!("Starting basic example");

    // 加载配置
    let config = match Config::load() {
        Ok(cfg) => {
            info!("Configuration loaded successfully");
            cfg
        }
        Err(e) => {
            error!("Failed to load configuration: {}", e);
            return Err(e.into());
        }
    };

    // 创建插件管理器
    let plugin_manager = Arc::new(PluginManager::new());
    debug!("Plugin manager initialized");

    // 创建并注册插件
    let hls_plugin = Arc::new(HLSPlugin::new(
        config.plugins.hls.cache_dir.to_string_lossy().to_string(),
    ));
    plugin_manager.register_plugin(hls_plugin.clone()).await?;
    info!("HLS plugin registered");

    // 使用 HLS 插件
    let stream_id = "test_stream";
    debug!("Adding test stream: {}", stream_id);
    hls_plugin.add_stream(stream_id.to_string(), 5000.0).await?;
    
    let url = "http://example.com/segment1.ts";
    info!("Attempting to download segment: {}", url);
    match hls_plugin.download_segment(url, stream_id).await {
        Ok(data) => info!("Successfully downloaded {} bytes", data.len()),
        Err(e) => warn!("Failed to download segment: {}", e),
    }

    // 健康检查
    let health_status = plugin_manager.health_check().await;
    info!("Plugin health status: {:?}", health_status);

    // 清理资源
    info!("Cleaning up resources");
    plugin_manager.cleanup().await?;
    info!("Example completed successfully");

    Ok(())
} 