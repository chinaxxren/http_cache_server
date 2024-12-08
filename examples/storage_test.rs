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

    info!("Starting storage test example");

    // 初始化存储插件
    let storage = Arc::new(StoragePlugin::new(
        "./test_storage".into(),
        1024 * 1024 * 10, // 10MB
    ));
    debug!("Storage plugin initialized with 10MB capacity");

    // 存储测试数据
    let test_data = vec![
        ("file1.txt", vec![1; 1024]),      // 1KB
        ("file2.txt", vec![2; 1024 * 100]), // 100KB
        ("file3.txt", vec![3; 1024 * 500]), // 500KB
    ];

    info!("Starting file storage tests");
    for (filename, data) in test_data {
        debug!("Storing file: {} ({} bytes)", filename, data.len());
        match storage.store_file(filename.to_string(), data).await {
            Ok(_) => {
                info!("Successfully stored file: {}", filename);
                // 获取文件统计信息
                if let Ok(stats) = storage.get_file_stats(filename).await {
                    debug!("File stats: size={}, last_access={:?}", stats.size, stats.last_access);
                }
            }
            Err(e) => error!("Failed to store file {}: {}", filename, e),
        }
    }

    // 检查存储指标
    if let Ok(metrics) = storage.get_storage_metrics().await {
        info!("Storage metrics:");
        info!("  - Total space: {} bytes", metrics.total_space);
        info!("  - Used space: {} bytes", metrics.used_space);
        info!("  - File count: {}", metrics.file_count);
        if let Some(oldest) = metrics.oldest_file {
            debug!("  - Oldest file age: {:?}", oldest.elapsed());
        }
    }

    // 测试文件检索
    let test_file = "file2.txt";
    info!("Testing file retrieval: {}", test_file);
    match storage.get_file(test_file).await {
        Ok(data) => {
            info!("Successfully retrieved file: {} ({} bytes)", test_file, data.len());
            if let Ok(stats) = storage.get_file_stats(test_file).await {
                debug!("Updated file stats after retrieval: {:?}", stats);
            }
        }
        Err(e) => warn!("Failed to retrieve file {}: {}", test_file, e),
    }

    // 测试过期清理
    info!("Testing expired files cleanup");
    tokio::time::sleep(Duration::from_secs(2)).await;
    match storage.cleanup_expired_files().await {
        Ok(_) => {
            info!("Cleanup completed successfully");
            // 检查清理后的状态
            if let Ok(metrics) = storage.get_storage_metrics().await {
                info!("Post-cleanup metrics:");
                info!("  - Used space: {} bytes", metrics.used_space);
                info!("  - File count: {}", metrics.file_count);
            }
        }
        Err(e) => error!("Cleanup failed: {}", e),
    }

    info!("Storage test example completed");
    Ok(())
} 