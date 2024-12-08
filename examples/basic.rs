use http_cache_server::prelude::*;
use http_cache_server::PluginManager;
use std::sync::Arc;
use tracing::{info, warn};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 初始化日志
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .init();

    info!("Starting example...");

    // 加载配置
    let config = Config::load()?;

    // 创建插件
    let hls_plugin = Arc::new(HLSPlugin::new(
        config.plugins.hls.cache_dir.to_string_lossy().to_string(),
    ));

    let storage_plugin = Arc::new(StoragePlugin::new(
        config.plugins.storage.root_path,
        config.plugins.storage.max_size,
    ));

    let security_plugin = Arc::new(SecurityPlugin::new(
        config.plugins.security.rate_limit,
    ));

    // 初始化插件管理器
    let plugin_manager = Arc::new(PluginManager::new());
    plugin_manager.register_plugin(hls_plugin.clone()).await?;
    plugin_manager.register_plugin(storage_plugin.clone()).await?;
    plugin_manager.register_plugin(security_plugin.clone()).await?;

    // 使用 HLS 插件
    hls_plugin.add_stream("stream1".to_string(), 5000.0).await?;
    
    let url = "http://example.com/segment1.ts";
    match hls_plugin.download_segment(url, "stream1").await {
        Ok(data) => info!("Downloaded {} bytes from {}", data.len(), url),
        Err(e) => warn!("Failed to download segment: {}", e),
    }

    // 使用存储插件
    let storage_status = storage_plugin.health_check().await?;
    info!("Storage health status: {}", storage_status);

    // 使用安全插件
    security_plugin.add_allowed_origin("https://example.com".to_string()).await;

    // 健康检查
    let health_status = plugin_manager.health_check().await;
    info!("Plugin health status: {:?}", health_status);

    // 清理资源
    plugin_manager.cleanup().await?;
    info!("Example completed");

    Ok(())
} 