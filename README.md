# HTTP Cache Server

一个基于插件架构的 HTTP 缓存服务器。

## 功能特性

- 插件化架构
- HLS 流媒体支持
- 存储管理
- 安全控制
- 配置灵活

## 运行

1. 使用默认配置运行：
```bash
cargo run
```

2. 使用自定义配置运行：
```bash
CONFIG_PATH=./config.toml cargo run
```

## 配置说明

### HLS 插件配置
```toml
[plugins.hls]
cache_dir = "./cache/hls"      # HLS 缓存目录
segment_duration = 10          # 分片时长(秒)
max_segments = 30             # 最大分片数
```

### 存储插件配置
```toml
[plugins.storage]
root_path = "./storage"       # 存储根目录
max_size = 1073741824        # 最大存储空间(1GB)
```

### 安全插件配置
```toml
[plugins.security]
rate_limit = 100             # 请求速率限制
allowed_origins = []         # 允许的来源
```

## 使用示例

### 基本使用

```rust
use http_cache_server::prelude::*;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 初始化插件
    let hls_plugin = Arc::new(HLSPlugin::new("./cache".to_string()));
    
    // 添加流
    hls_plugin.add_stream("stream1".to_string(), 5000.0).await?;
    
    // 下载分片
    let data = hls_plugin.download_segment(
        "http://example.com/segment1.ts",
        "stream1"
    ).await?;
    
    // 更新带宽
    hls_plugin.update_bandwidth("stream1", 6000.0).await?;
    
    Ok(())
}
```

### 高级功能

```rust
// 配置网络重试
let plugin = Arc::new(HLSPlugin::new("./cache".to_string()));
plugin.set_max_retries(5).await;
plugin.set_timeout(Duration::from_secs(60)).await;

// 使用存储插件
let storage = Arc::new(StoragePlugin::new("./storage".into(), 1024 * 1024 * 1024));
storage.cleanup_expired().await?;

// 使用安全插件
let security = Arc::new(SecurityPlugin::new(100));
security.add_allowed_origin("https://example.com".to_string()).await;
```

## 插件开发

实现 Plugin trait 来创建新插件：

```rust
#[async_trait]
impl Plugin for MyPlugin {
    fn name(&self) -> &str {
        "my_plugin"
    }

    fn version(&self) -> &str {
        "1.0.0"
    }

    async fn init(&self) -> Result<(), PluginError> {
        // 初始化逻辑
        Ok(())
    }

    async fn cleanup(&self) -> Result<(), PluginError> {
        // 清理逻辑
        Ok(())
    }

    async fn health_check(&self) -> Result<bool, PluginError> {
        // 健康检查逻辑
        Ok(true)
    }
}
```

## 错误处理

```rust
use http_cache_server::error::PluginError;

match plugin.download_segment(url, stream_id).await {
    Ok(data) => println!("Downloaded {} bytes", data.len()),
    Err(PluginError::Network(e)) => eprintln!("Network error: {}", e),
    Err(PluginError::Storage(e)) => eprintln!("Storage error: {}", e),
    Err(e) => eprintln!("Other error: {}", e),
}
```

## 日志记录

服务器使用 tracing 进行日志记录：

```rust
use tracing::{info, warn, error};

// 初始化日志
tracing_subscriber::fmt()
    .with_env_filter("info")
    .init();

// 记录日志
info!("Starting server...");
warn!("Resource usage high");
error!("Failed to process request: {}", error);
```