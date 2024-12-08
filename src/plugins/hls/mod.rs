use crate::plugin::Plugin;
use crate::error::PluginError;
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::RwLock;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use super::network::NetworkManager;
use super::bandwidth::BandwidthManager;
use tracing::{info, warn, error, debug, instrument};

mod adaptive;
mod segment;

use adaptive::AdaptiveStreamManager;
use segment::SegmentManager;

#[derive(Debug)]
pub struct HLSPlugin {
    config: HLSConfig,
    state: Arc<RwLock<HLSState>>,
    adaptive_manager: Arc<AdaptiveStreamManager>,
    segment_manager: Arc<SegmentManager>,
    network_manager: Arc<NetworkManager>,
    bandwidth_manager: Arc<BandwidthManager>,
}

#[derive(Debug)]
struct HLSConfig {
    cache_dir: String,
    segment_duration: u32,
    max_segments: usize,
    bandwidth_check_interval: Duration,
}

#[derive(Debug)]
struct HLSState {
    active_streams: HashMap<String, StreamInfo>,
    _last_cleanup: Instant,
}

#[derive(Debug, Clone)]
pub struct StreamInfo {
    pub last_access: Instant,
    pub segments: Vec<String>,
    pub bandwidth: f64,
}

impl HLSPlugin {
    pub fn new(cache_dir: String) -> Self {
        info!("Initializing HLSPlugin with cache directory: {}", cache_dir);
        let config = HLSConfig {
            cache_dir: cache_dir.clone(),
            segment_duration: 10,
            max_segments: 30,
            bandwidth_check_interval: Duration::from_secs(5),
        };
        debug!("HLS configuration: {:?}", config);

        let state = Arc::new(RwLock::new(HLSState {
            active_streams: HashMap::new(),
            _last_cleanup: Instant::now(),
        }));

        let adaptive_manager = Arc::new(AdaptiveStreamManager::new());
        let segment_manager = Arc::new(SegmentManager::new(cache_dir));
        let network_manager = Arc::new(NetworkManager::new());
        let bandwidth_manager = Arc::new(BandwidthManager::new());

        info!("HLS plugin initialized successfully");
        Self {
            config,
            state,
            adaptive_manager,
            segment_manager,
            network_manager,
            bandwidth_manager,
        }
    }

    pub async fn get_active_stream_count(&self) -> usize {
        let state = self.state.read().await;
        let count = state.active_streams.len();
        debug!("Current active stream count: {}", count);
        count
    }

    pub async fn get_stream_info(&self, stream_id: &str) -> Option<StreamInfo> {
        let state = self.state.read().await;
        let info = state.active_streams.get(stream_id).cloned();
        if let Some(ref stream) = info {
            debug!(
                "Stream info for {}: bandwidth={}, segments={}", 
                stream_id, stream.bandwidth, stream.segments.len()
            );
        } else {
            debug!("Stream {} not found", stream_id);
        }
        info
    }

    pub async fn cleanup_old_streams(&self) -> Result<(), PluginError> {
        info!("Starting stream cleanup");
        let mut state = self.state.write().await;
        let now = Instant::now();
        let timeout = Duration::from_secs(3600);

        let before_count = state.active_streams.len();
        state.active_streams.retain(|id, stream| {
            let keep = now.duration_since(stream.last_access) < timeout;
            if !keep {
                info!("Removing inactive stream: {}", id);
            }
            keep
        });
        let after_count = state.active_streams.len();
        
        info!("Stream cleanup completed. Removed {} streams", before_count - after_count);
        Ok(())
    }

    pub async fn get_stream_metrics(&self) -> Result<StreamMetrics, PluginError> {
        let state = self.state.read().await;
        let metrics = StreamMetrics {
            active_streams: state.active_streams.len(),
            total_segments: state.active_streams.values()
                .map(|s| s.segments.len())
                .sum(),
            avg_bandwidth: if !state.active_streams.is_empty() {
                state.active_streams.values()
                    .map(|s| s.bandwidth)
                    .sum::<f64>() / state.active_streams.len() as f64
            } else {
                0.0
            },
        };
        debug!("Stream metrics: {:?}", metrics);
        Ok(metrics)
    }

    pub async fn add_stream(&self, stream_id: String, bandwidth: f64) -> Result<(), PluginError> {
        info!("Adding stream {} with bandwidth {}", stream_id, bandwidth);
        let mut state = self.state.write().await;
        state.active_streams.insert(stream_id.clone(), StreamInfo {
            last_access: Instant::now(),
            segments: Vec::new(),
            bandwidth,
        });
        
        self.adaptive_manager.register_stream(stream_id.clone(), bandwidth).await;
        info!("Stream {} added successfully", stream_id);
        Ok(())
    }

    pub async fn update_bandwidth(&self, stream_id: &str, bandwidth: f64) -> Result<(), PluginError> {
        debug!("Updating bandwidth for stream {} to {} bps", stream_id, bandwidth);
        let mut state = self.state.write().await;
        if let Some(stream) = state.active_streams.get_mut(stream_id) {
            stream.bandwidth = bandwidth;
            stream.last_access = Instant::now();
            info!("Bandwidth updated successfully for stream {}", stream_id);
        } else {
            warn!("Stream {} not found for bandwidth update", stream_id);
            return Err(PluginError::Hls(format!("Stream {} not found", stream_id)));
        }
        
        self.adaptive_manager.update_bandwidth(stream_id, bandwidth).await;
        Ok(())
    }

    #[instrument(skip(self))]
    async fn handle_segment_download(&self, url: &str, stream_id: &str) -> Result<bytes::Bytes, PluginError> {
        debug!("Starting segment download process for {}", url);
        let mut state = self.state.write().await;
        
        // 检查流是否存在
        if !state.active_streams.contains_key(stream_id) {
            error!("Stream {} not found", stream_id);
            return Err(PluginError::Hls(format!("Stream {} not found", stream_id)));
        }

        // 更新访问时间
        if let Some(stream) = state.active_streams.get_mut(stream_id) {
            stream.last_access = Instant::now();
            debug!("Updated last access time for stream {}", stream_id);
        }

        // 检查分片数量限制
        let stream = state.active_streams.get_mut(stream_id).unwrap();
        if stream.segments.len() >= self.config.max_segments {
            warn!(
                "Maximum segment count ({}) reached for stream {}",
                self.config.max_segments, stream_id
            );
            // 移除最旧的分片
            if let Some(oldest_segment) = stream.segments.first().cloned() {
                debug!("Removing oldest segment: {}", oldest_segment);
                stream.segments.remove(0);
            }
        }

        // 下载分片
        let data = self.network_manager.get(url).await?;
        
        // 更新分片列表
        stream.segments.push(url.to_string());
        info!(
            "Added new segment to stream {}. Total segments: {}",
            stream_id,
            stream.segments.len()
        );

        Ok(data)
    }

    #[instrument(skip(self))]
    pub async fn get_segment_stats(&self, stream_id: &str) -> Result<SegmentStats, PluginError> {
        let state = self.state.read().await;
        if let Some(stream) = state.active_streams.get(stream_id) {
            let stats = SegmentStats {
                total_segments: stream.segments.len(),
                current_bandwidth: stream.bandwidth,
                last_access: stream.last_access,
            };
            debug!("Segment stats for stream {}: {:?}", stream_id, stats);
            Ok(stats)
        } else {
            warn!("Failed to get segment stats: stream {} not found", stream_id);
            Err(PluginError::Hls(format!("Stream {} not found", stream_id)))
        }
    }

    pub async fn download_segment(&self, url: &str, stream_id: &str) -> Result<bytes::Bytes, PluginError> {
        debug!("Starting segment download for stream {} from {}", stream_id, url);
        
        // 检查带宽限制
        if let Err(e) = self.bandwidth_manager.check_limit(stream_id, 0).await {
            warn!("Bandwidth limit check failed for stream {}: {}", stream_id, e);
            return Err(e);
        }
        
        // 下载分片
        match self.handle_segment_download(url, stream_id).await {
            Ok(data) => {
                let size = data.len();
                info!(
                    "Successfully downloaded segment for stream {}. Size: {} bytes",
                    stream_id, size
                );
                
                // 更新带宽使用
                if let Err(e) = self.bandwidth_manager.check_limit(stream_id, size as u64).await {
                    warn!(
                        "Bandwidth limit exceeded after download for stream {}: {}",
                        stream_id, e
                    );
                }
                
                // 更新分片管理器
                self.segment_manager.add_segment(url.to_string(), size as u64).await;
                debug!(
                    "Updated segment manager for stream {}. URL: {}, Size: {}",
                    stream_id, url, size
                );
                
                Ok(data)
            }
            Err(e) => {
                error!(
                    "Failed to download segment for stream {} from {}: {}",
                    stream_id, url, e
                );
                Err(e)
            }
        }
    }

    pub async fn get_performance_metrics(&self) -> Result<PerformanceMetrics, PluginError> {
        let state = self.state.read().await;
        let now = Instant::now();
        
        let metrics = PerformanceMetrics {
            active_streams: state.active_streams.len(),
            total_segments: state.active_streams.values()
                .map(|s| s.segments.len())
                .sum(),
            avg_bandwidth: if !state.active_streams.is_empty() {
                state.active_streams.values()
                    .map(|s| s.bandwidth)
                    .sum::<f64>() / state.active_streams.len() as f64
            } else {
                0.0
            },
            oldest_stream: state.active_streams.values()
                .map(|s| now.duration_since(s.last_access))
                .max(),
        };

        info!("Performance metrics: {:?}", metrics);
        Ok(metrics)
    }

    pub async fn get_stream_bandwidth(&self, stream_id: &str) -> Option<f64> {
        if let Some(info) = self.get_stream_info(stream_id).await {
            Some(info.bandwidth)
        } else {
            None
        }
    }

    pub async fn get_stream_segments_count(&self, stream_id: &str) -> Option<usize> {
        if let Some(info) = self.get_stream_info(stream_id).await {
            Some(info.segments.len())
        } else {
            None
        }
    }
}

#[derive(Debug)]
pub struct StreamMetrics {
    pub active_streams: usize,
    pub total_segments: usize,
    pub avg_bandwidth: f64,
}

#[derive(Debug)]
pub struct SegmentStats {
    pub total_segments: usize,
    pub current_bandwidth: f64,
    pub last_access: Instant,
}

#[derive(Debug)]
pub struct PerformanceMetrics {
    pub active_streams: usize,
    pub total_segments: usize,
    pub avg_bandwidth: f64,
    pub oldest_stream: Option<Duration>,
}

#[async_trait]
impl Plugin for HLSPlugin {
    fn name(&self) -> &str {
        "hls"
    }

    fn version(&self) -> &str {
        "1.0.0"
    }

    async fn init(&self) -> Result<(), PluginError> {
        info!("Initializing HLS plugin");
        match tokio::fs::create_dir_all(&self.config.cache_dir).await {
            Ok(_) => debug!("Created cache directory: {:?}", self.config.cache_dir),
            Err(e) => {
                error!("Failed to create cache directory: {}", e);
                return Err(PluginError::Hls(e.to_string()));
            }
        }
        
        // Start background cleanup task
        let plugin = self.clone();
        let cleanup_interval = Duration::from_secs(300);
        info!("Starting background cleanup task with interval: {:?}", cleanup_interval);
        
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(cleanup_interval).await;
                debug!("Running scheduled stream cleanup");
                match plugin.cleanup_old_streams().await {
                    Ok(_) => debug!("Background cleanup completed successfully"),
                    Err(e) => error!("Error during background cleanup: {}", e),
                }
            }
        });

        info!("HLS plugin initialization completed");
        Ok(())
    }

    async fn cleanup(&self) -> Result<(), PluginError> {
        info!("Starting HLS plugin cleanup");
        let state = self.state.read().await;
        let mut removed_count = 0;
        let mut failed_count = 0;

        for (stream_id, stream) in state.active_streams.iter() {
            debug!("Cleaning up stream {}", stream_id);
            for segment in &stream.segments {
                match tokio::fs::remove_file(segment).await {
                    Ok(_) => {
                        removed_count += 1;
                        debug!("Removed segment file: {}", segment);
                    }
                    Err(e) => {
                        failed_count += 1;
                        warn!("Failed to remove segment {}: {}", segment, e);
                    }
                }
            }
        }

        info!(
            "HLS plugin cleanup completed. Removed: {}, Failed: {}",
            removed_count, failed_count
        );
        Ok(())
    }

    async fn health_check(&self) -> Result<bool, PluginError> {
        let state = self.state.read().await;
        let stream_count = state.active_streams.len();
        let total_segments: usize = state.active_streams.values()
            .map(|s| s.segments.len())
            .sum();
        
        let healthy = !state.active_streams.is_empty();
        if healthy {
            info!(
                "HLS plugin health check passed. Active streams: {}, Total segments: {}",
                stream_count, total_segments
            );
        } else {
            warn!("HLS plugin health check failed. No active streams");
        }
        
        Ok(healthy)
    }
}

// 为了支持后台任务的克隆
impl Clone for HLSPlugin {
    fn clone(&self) -> Self {
        Self {
            config: HLSConfig {
                cache_dir: self.config.cache_dir.clone(),
                segment_duration: self.config.segment_duration,
                max_segments: self.config.max_segments,
                bandwidth_check_interval: self.config.bandwidth_check_interval,
            },
            state: self.state.clone(),
            adaptive_manager: self.adaptive_manager.clone(),
            segment_manager: self.segment_manager.clone(),
            network_manager: self.network_manager.clone(),
            bandwidth_manager: self.bandwidth_manager.clone(),
        }
    }
} 