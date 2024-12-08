use std::sync::Arc;
use std::net::SocketAddr;
use http_cache_server::{plugin_manager::PluginManager, prelude::*};
use tracing::{info, warn, error, debug, instrument};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 初始化日志
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .with_file(true)
        .with_line_number(true)
        .with_thread_ids(true)
        .init();
    
    info!("Starting HTTP Cache Server");
    
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

    // 创建并注册 HLS 插件
    let hls_plugin = Arc::new(HLSPlugin::new(
        config.plugins.hls.cache_dir.to_string_lossy().to_string(),
    ));
    if let Err(e) = plugin_manager.register_plugin(hls_plugin.clone()).await {
        error!("Failed to register HLS plugin: {}", e);
        return Err(e.into());
    }
    info!("HLS plugin registered successfully");

    // 创建并注册代理服务器
    let addr: SocketAddr = match format!("{}:{}", config.server.host, config.server.port).parse() {
        Ok(addr) => addr,
        Err(e) => {
            error!("Invalid server address configuration: {}", e);
            return Err(e.into());
        }
    };
    
    let proxy_server = Arc::new(ProxyServer::new(addr, hls_plugin));
    if let Err(e) = plugin_manager.register_plugin(proxy_server.clone()).await {
        error!("Failed to register proxy server: {}", e);
        return Err(e.into());
    }
    info!("Proxy server registered successfully");

    // 启动健康检查
    start_health_check(plugin_manager.clone());
    info!("Health check service started");

    // 运行代理服务器
    info!("Starting proxy server");
    if let Err(e) = proxy_server.run().await {
        error!("Proxy server error: {}", e);
        return Err(e.into());
    }

    // 优雅关闭
    info!("Initiating graceful shutdown");
    if let Err(e) = plugin_manager.cleanup().await {
        error!("Error during shutdown: {}", e);
        return Err(e.into());
    }
    info!("Shutdown completed successfully");
    
    Ok(())
}

#[instrument]
fn start_health_check(plugin_manager: Arc<PluginManager>) {
    info!("Starting health check service");
    tokio::spawn(async move {
        let check_interval = std::time::Duration::from_secs(300); // 5 minutes
        loop {
            let health_status = plugin_manager.health_check().await;
            info!("Health check results: {:?}", health_status);
            
            for (plugin, status) in &health_status {
                if !status {
                    warn!("Plugin {} health check failed", plugin);
                } else {
                    debug!("Plugin {} health check passed", plugin);
                }
            }
            
            tokio::time::sleep(check_interval).await;
        }
    });
} 