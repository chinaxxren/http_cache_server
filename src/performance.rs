use std::sync::Arc;
use tokio::sync::RwLock;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use crate::error::PluginError;
use tracing::{info, warn, debug};

#[derive(Debug)]
pub struct PerformanceOptimizer {
    state: Arc<RwLock<OptimizerState>>,
    config: OptimizerConfig,
}

#[derive(Debug)]
struct OptimizerState {
    metrics: HashMap<String, ResourceMetrics>,
    thresholds: HashMap<String, ThresholdConfig>,
}

#[derive(Debug)]
struct ResourceMetrics {
    response_times: Vec<Duration>,
    error_count: usize,
    last_optimized: Instant,
    current_load: f64,
}

#[derive(Debug)]
pub struct OptimizerConfig {
    pub max_response_time: Duration,
    pub error_threshold: usize,
    pub optimization_interval: Duration,
    pub load_threshold: f64,
}

#[derive(Debug)]
struct ThresholdConfig {
    max_concurrent: usize,
    rate_limit: u32,
    timeout: Duration,
}

impl PerformanceOptimizer {
    pub fn new(config: OptimizerConfig) -> Self {
        info!("Initializing performance optimizer with config: {:?}", config);
        Self {
            state: Arc::new(RwLock::new(OptimizerState {
                metrics: HashMap::new(),
                thresholds: HashMap::new(),
            })),
            config,
        }
    }

    pub async fn record_request(
        &self,
        resource_id: &str,
        response_time: Duration,
        success: bool,
    ) {
        debug!(
            "Recording request for {}: time={:?}, success={}",
            resource_id, response_time, success
        );

        let mut state = self.state.write().await;
        let metrics = state.metrics.entry(resource_id.to_string())
            .or_insert_with(|| ResourceMetrics {
                response_times: Vec::new(),
                error_count: 0,
                last_optimized: Instant::now(),
                current_load: 0.0,
            });

        metrics.response_times.push(response_time);
        if !success {
            metrics.error_count += 1;
        }

        // 保持最近的响应时间记录
        if metrics.response_times.len() > 100 {
            metrics.response_times.remove(0);
        }

        // 检查是否需要优化
        if self.should_optimize(metrics) {
            info!("Triggering optimization for resource: {}", resource_id);
            self.optimize_resource(resource_id, metrics).await;
        }
    }

    fn should_optimize(&self, metrics: &ResourceMetrics) -> bool {
        let avg_response_time = metrics.response_times.iter()
            .sum::<Duration>() / metrics.response_times.len() as u32;

        avg_response_time > self.config.max_response_time ||
        metrics.error_count >= self.config.error_threshold ||
        metrics.current_load > self.config.load_threshold
    }

    async fn optimize_resource(&self, resource_id: &str, metrics: &mut ResourceMetrics) {
        debug!("Optimizing resource: {}", resource_id);

        // 计算新的阈值
        let new_threshold = self.calculate_thresholds(metrics);
        
        // 更新阈值配置
        let mut state = self.state.write().await;
        state.thresholds.insert(resource_id.to_string(), new_threshold);

        // 重置指标
        metrics.error_count = 0;
        metrics.last_optimized = Instant::now();

        info!(
            "Resource {} optimized. New thresholds configured.",
            resource_id
        );
    }

    fn calculate_thresholds(&self, metrics: &ResourceMetrics) -> ThresholdConfig {
        let avg_response_time = metrics.response_times.iter()
            .sum::<Duration>() / metrics.response_times.len() as u32;

        let max_concurrent = if avg_response_time > self.config.max_response_time {
            // 减少并发连接数
            20
        } else {
            50
        };

        let rate_limit = if metrics.error_count > self.config.error_threshold {
            // 降低请求速率
            100
        } else {
            200
        };

        let timeout = if metrics.current_load > self.config.load_threshold {
            // 增加超时时间
            Duration::from_secs(30)
        } else {
            Duration::from_secs(10)
        };

        ThresholdConfig {
            max_concurrent,
            rate_limit,
            timeout,
        }
    }

    pub async fn get_resource_metrics(&self, resource_id: &str) -> Option<MetricsSnapshot> {
        let state = self.state.read().await;
        state.metrics.get(resource_id).map(|metrics| {
            let avg_response_time = metrics.response_times.iter()
                .sum::<Duration>() / metrics.response_times.len() as u32;

            MetricsSnapshot {
                avg_response_time,
                error_rate: metrics.error_count as f64 / metrics.response_times.len() as f64,
                current_load: metrics.current_load,
                last_optimized: metrics.last_optimized,
            }
        })
    }

    pub async fn update_load(&self, resource_id: &str, load: f64) -> Result<(), PluginError> {
        debug!("Updating load for {}: {}", resource_id, load);
        let mut state = self.state.write().await;
        
        if let Some(metrics) = state.metrics.get_mut(resource_id) {
            metrics.current_load = load;
            Ok(())
        } else {
            warn!("Resource not found: {}", resource_id);
            Err(PluginError::Network(format!("Resource not found: {}", resource_id)))
        }
    }

    pub async fn get_thresholds(&self, resource_id: &str) -> Option<ThresholdSnapshot> {
        let state = self.state.read().await;
        state.thresholds.get(resource_id).map(|config| ThresholdSnapshot {
            max_concurrent: config.max_concurrent,
            rate_limit: config.rate_limit,
            timeout: config.timeout,
        })
    }
}

#[derive(Debug)]
pub struct MetricsSnapshot {
    pub avg_response_time: Duration,
    pub error_rate: f64,
    pub current_load: f64,
    pub last_optimized: Instant,
}

#[derive(Debug, Clone)]
pub struct ThresholdSnapshot {
    pub max_concurrent: usize,
    pub rate_limit: u32,
    pub timeout: Duration,
}

impl Default for OptimizerConfig {
    fn default() -> Self {
        Self {
            max_response_time: Duration::from_secs(2),
            error_threshold: 5,
            optimization_interval: Duration::from_secs(300),
            load_threshold: 0.8,
        }
    }
} 