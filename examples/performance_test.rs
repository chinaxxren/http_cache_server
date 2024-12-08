use http_cache_server::prelude::*;
use http_cache_server::performance::{PerformanceOptimizer, OptimizerConfig};
use std::time::Duration;
use tracing::{info, warn, debug};
use std::error::Error;
use tokio::time::sleep;
use rand::Rng;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    // 初始化日志
    tracing_subscriber::fmt()
        .with_env_filter("debug")
        .with_file(true)
        .with_line_number(true)
        .init();

    info!("Starting performance optimization test");

    // 创建性能优化器
    let optimizer = PerformanceOptimizer::new(OptimizerConfig {
        max_response_time: Duration::from_millis(500),
        error_threshold: 3,
        optimization_interval: Duration::from_secs(60),
        load_threshold: 0.7,
    });

    // 模拟多个资源的请求
    let resources = vec![
        "api/users",
        "api/products",
        "api/orders",
    ];

    // 模拟不同负载场景
    let scenarios = vec![
        ("Normal Load", 0.3, 100_i64, 0.01),   // 正常负载
        ("High Load", 0.7, 300_i64, 0.05),     // 高负载
        ("Peak Load", 0.9, 500_i64, 0.1),      // 峰值负载
    ];

    let mut rng = rand::thread_rng();

    for (scenario_name, load, base_latency, error_rate) in scenarios {
        info!("Starting scenario: {}", scenario_name);

        // 更新资源负载
        for resource in &resources {
            optimizer.update_load(resource, load).await?;
            debug!("Set load to {} for resource: {}", load, resource);
        }

        // 模拟请求
        for _ in 0..50 {
            for resource in &resources {
                // 模拟响应时间波动
                let latency_variance = rng.gen_range(-50_i64..=100_i64);
                let response_time = Duration::from_millis(
                    (base_latency + latency_variance).max(0) as u64
                );

                // 模拟随机错误
                let success = rng.gen::<f64>() > error_rate;

                // 记录请求
                optimizer.record_request(resource, response_time, success).await;

                // 获取并打印指标
                if let Some(metrics) = optimizer.get_resource_metrics(resource).await {
                    info!(
                        "Resource: {}, Avg Response Time: {:?}, Error Rate: {:.2}%, Load: {:.2}",
                        resource,
                        metrics.avg_response_time,
                        metrics.error_rate * 100.0,
                        metrics.current_load
                    );
                }

                // 获取优化后的阈值
                if let Some(thresholds) = optimizer.get_thresholds(resource).await {
                    debug!(
                        "Resource: {}, Max Concurrent: {}, Rate Limit: {}, Timeout: {:?}",
                        resource,
                        thresholds.max_concurrent,
                        thresholds.rate_limit,
                        thresholds.timeout
                    );
                }

                // 模拟请求间隔
                sleep(Duration::from_millis(100)).await;
            }
        }

        // 场景之间的间隔
        info!("Completed scenario: {}", scenario_name);
        sleep(Duration::from_secs(2)).await;
    }

    // 打印最终统计
    info!("Performance test completed");
    for resource in &resources {
        if let Some(metrics) = optimizer.get_resource_metrics(resource).await {
            info!("Final metrics for {}:", resource);
            info!("  - Average response time: {:?}", metrics.avg_response_time);
            info!("  - Error rate: {:.2}%", metrics.error_rate * 100.0);
            info!("  - Current load: {:.2}", metrics.current_load);
            info!("  - Last optimized: {:?} ago", metrics.last_optimized.elapsed());
        }

        if let Some(thresholds) = optimizer.get_thresholds(resource).await {
            info!("Final thresholds for {}:", resource);
            info!("  - Max concurrent requests: {}", thresholds.max_concurrent);
            info!("  - Rate limit: {}/s", thresholds.rate_limit);
            info!("  - Timeout: {:?}", thresholds.timeout);
        }
    }

    Ok(())
} 