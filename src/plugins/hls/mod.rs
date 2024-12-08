use crate::plugin::Plugin;
use crate::error::PluginError;
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::RwLock;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use super::network::NetworkManager;
use super::bandwidth::BandwidthManager;

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

#[derive(Debug)]
struct StreamInfo {
    last_access: Instant,
    segments: Vec<String>,
    bandwidth: f64,
}

impl HLSPlugin {
    pub fn new(cache_dir: String) -> Self {
        let config = HLSConfig {
            cache_dir: cache_dir.clone(),
            segment_duration: 10,
            max_segments: 30,
            bandwidth_check_interval: Duration::from_secs(5),
        };

        let state = Arc::new(RwLock::new(HLSState {
            active_streams: HashMap::new(),
            _last_cleanup: Instant::now(),
        }));

        let adaptive_manager = Arc::new(AdaptiveStreamManager::new());
        let segment_manager = Arc::new(SegmentManager::new(cache_dir));
        let network_manager = Arc::new(NetworkManager::new());
        let bandwidth_manager = Arc::new(BandwidthManager::new());

        Self {
            config,
            state,
            adaptive_manager,
            segment_manager,
            network_manager,
            bandwidth_manager,
        }
    }

    pub async fn add_stream(&self, stream_id: String, bandwidth: f64) -> Result<(), PluginError> {
        let mut state = self.state.write().await;
        state.active_streams.insert(stream_id.clone(), StreamInfo {
            last_access: Instant::now(),
            segments: Vec::new(),
            bandwidth,
        });
        
        self.adaptive_manager.register_stream(stream_id, bandwidth).await;
        Ok(())
    }

    pub async fn update_bandwidth(&self, stream_id: &str, bandwidth: f64) -> Result<(), PluginError> {
        let mut state = self.state.write().await;
        if let Some(stream) = state.active_streams.get_mut(stream_id) {
            stream.bandwidth = bandwidth;
            stream.last_access = Instant::now();
        }
        
        self.adaptive_manager.update_bandwidth(stream_id, bandwidth).await;
        Ok(())
    }

    async fn cleanup_old_streams(&self) -> Result<(), PluginError> {
        let mut state = self.state.write().await;
        let now = Instant::now();
        let timeout = Duration::from_secs(3600); // 1 hour

        state.active_streams.retain(|_, stream| {
            now.duration_since(stream.last_access) < timeout
        });

        Ok(())
    }

    pub async fn download_segment(&self, url: &str, stream_id: &str) -> Result<bytes::Bytes, PluginError> {
        // 检查带宽限制
        self.bandwidth_manager.check_limit(stream_id, 0).await?;
        
        // 下载分片
        let data = self.network_manager.get(url).await?;
        
        // 更新带宽使用
        self.bandwidth_manager.check_limit(stream_id, data.len() as u64).await?;
        
        // 更新分片管理器
        self.segment_manager.add_segment(
            url.to_string(),
            data.len() as u64,
        ).await;

        Ok(data)
    }
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
        tokio::fs::create_dir_all(&self.config.cache_dir).await?;
        
        // Start background cleanup task
        let plugin = self.clone();
        tokio::spawn(async move {
            let cleanup_interval = Duration::from_secs(300); // 5 minutes
            loop {
                tokio::time::sleep(cleanup_interval).await;
                if let Err(e) = plugin.cleanup_old_streams().await {
                    tracing::error!("Error during stream cleanup: {}", e);
                }
            }
        });

        Ok(())
    }

    async fn cleanup(&self) -> Result<(), PluginError> {
        let state = self.state.read().await;
        for (_, stream) in state.active_streams.iter() {
            for segment in &stream.segments {
                let _ = tokio::fs::remove_file(segment).await;
            }
        }
        Ok(())
    }

    async fn health_check(&self) -> Result<bool, PluginError> {
        let state = self.state.read().await;
        Ok(!state.active_streams.is_empty())
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