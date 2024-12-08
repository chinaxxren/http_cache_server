use http_cache_server::{plugins::cache::{CacheConfig, CacheMetadata}, prelude::*};
use std::sync::Arc;
use tracing::{info, warn, debug};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 初始化日志
    tracing_subscriber::fmt()
        .with_env_filter("debug")
        .with_file(true)
        .with_line_number(true)
        .init();

    info!("Starting cache test");

    // 创建缓存管理器
    let cache = Arc::new(CacheManager::new(
        "./test_cache",
        CacheConfig {
            max_space: 1024 * 1024 * 10, // 10MB
            cleanup_interval: Duration::from_secs(60),
            entry_ttl: Duration::from_secs(3600),
            min_free_space: 1024 * 1024, // 1MB
        }
    ));

    // 存储测试数据
    let test_data = vec![
        ("key1", "Hello World!".as_bytes().to_vec()),
        ("key2", vec![1, 2, 3, 4, 5]),
        ("key3", "Test Data".as_bytes().to_vec()),
    ];

    for (key, data) in test_data {
        let metadata = CacheMetadata::new("application/octet-stream".into())
            .with_etag("test-etag".into());

        match cache.store(key.to_string(), data.clone(), metadata).await {
            Ok(_) => info!("Successfully stored data for key: {}", key),
            Err(e) => warn!("Failed to store data for key {}: {}", key, e),
        }
    }

    // 读取测试
    for key in ["key1", "key2", "key3"] {
        match cache.get(key).await {
            Ok((data, metadata)) => {
                info!(
                    "Retrieved {} bytes for key {} (etag: {:?})",
                    data.len(),
                    key,
                    metadata.etag
                );
            }
            Err(e) => warn!("Failed to retrieve key {}: {}", key, e),
        }
    }

    // 获取缓存统计
    match cache.get_stats().await {
        Ok(stats) => {
            info!("Cache stats:");
            info!("  - Total entries: {}", stats.total_entries);
            info!("  - Used space: {} bytes", stats.used_space);
            info!("  - Usage: {:.1}%", stats.usage_percent);
        }
        Err(e) => warn!("Failed to get cache stats: {}", e),
    }

    // 清理测试
    match cache.cleanup().await {
        Ok(_) => info!("Cache cleanup completed successfully"),
        Err(e) => warn!("Cache cleanup failed: {}", e),
    }

    info!("Cache test completed");
    Ok(())
} 