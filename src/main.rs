use std::sync::Arc;
use http_cache_server::prelude::*;
use http_cache_server::PluginManager;
use tracing::{info, error};
use tokio::signal;
use http_cache_server::Config;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 初始化日志
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .init();
    
    info!("Starting HTTP Cache Server");
    
    // 创建插件管理器
    let plugin_manager = Arc::new(PluginManager::new());

    // 初始化配置
    let config = load_config()?;
    
    // 初始化并注册插件
    let plugins = create_plugins(&config)?;
    for plugin in plugins {
        if let Err(e) = plugin_manager.register_plugin(plugin).await {
            error!("Failed to register plugin: {}", e);
        }
    }

    // 启动健康检查
    start_health_check(plugin_manager.clone());

    // 等待关闭信号
    wait_for_shutdown().await;
    
    // 优雅关闭
    info!("Shutting down...");
    if let Err(e) = plugin_manager.cleanup().await {
        error!("Error during shutdown: {}", e);
    }
    info!("Shutdown complete");
    
    Ok(())
}

fn load_config() -> Result<Config, Box<dyn std::error::Error>> {
    Config::load()
}

fn create_plugins(config: &Config) -> Result<Vec<Arc<dyn Plugin>>, Box<dyn std::error::Error>> {
    let mut plugins: Vec<Arc<dyn Plugin>> = Vec::new();
    
    // HLS Plugin
    let hls_plugin = Arc::new(HLSPlugin::new(
        config.plugins.hls.cache_dir.to_string_lossy().to_string(),
    ));
    plugins.push(hls_plugin);

    // Storage Plugin
    let storage_plugin = Arc::new(StoragePlugin::new(
        config.plugins.storage.root_path.clone(),
        config.plugins.storage.max_size,
    ));
    plugins.push(storage_plugin);

    // Security Plugin
    let security_plugin = Arc::new(SecurityPlugin::new(
        config.plugins.security.rate_limit,
    ));
    plugins.push(security_plugin);

    Ok(plugins)
}

fn start_health_check(plugin_manager: Arc<PluginManager>) {
    tokio::spawn(async move {
        let check_interval = std::time::Duration::from_secs(300); // 5 minutes
        loop {
            let health_status = plugin_manager.health_check().await;
            info!("Plugin health status: {:?}", health_status);
            tokio::time::sleep(check_interval).await;
        }
    });
}

async fn wait_for_shutdown() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("Failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => info!("Received Ctrl+C signal"),
        _ = terminate => info!("Received terminate signal"),
    }
} 