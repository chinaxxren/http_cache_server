use std::sync::Arc;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use tokio::sync::{RwLock, mpsc};
use tracing::{info, warn, error, debug, instrument};
use bytes::Bytes;
use uuid::Uuid;
use async_trait::async_trait;
use m3u8_rs::{MasterPlaylist, MediaPlaylist, parse_master_playlist, parse_media_playlist};

use crate::error::PluginError;
use crate::plugin::Plugin;
use crate::plugins::cache::{CacheManager, CacheMetadata};

mod segment;
use segment::SegmentManager;

#[derive(Debug)]
pub struct HLSPlugin {
    config: HLSConfig,
    state: Arc<RwLock<HLSState>>,
    segment_manager: Arc<SegmentManager>,
    cache: Arc<CacheManager>,
    prefetch_tx: mpsc::Sender<String>,
}

#[derive(Debug, Default)]
struct HLSConfig {
    cache_dir: String,
    segment_duration: u32,
    max_segments: usize,
    bandwidth_check_interval: Duration,
}

#[derive(Debug)]
struct HLSState {
    active_streams: HashMap<String, StreamInfo>,
}

impl Default for HLSState {
    fn default() -> Self {
        Self {
            active_streams: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct StreamInfo {
    pub last_access: Instant,
    pub segments: Vec<String>,
    pub bandwidth: f64,
}

impl HLSPlugin {
    pub fn new(cache_dir: String, cache: Arc<CacheManager>) -> Self {
        info!("Initializing HLSPlugin with cache directory: {}", cache_dir);
        
        // 创建预取通道
        let (tx, mut rx) = mpsc::channel(100);
        let tx_clone = tx.clone();  // 克隆一个发送器用于异步任务
        
        // 启动预取处理任务
        let cache_clone = cache.clone();
        let segment_manager_clone = Arc::new(SegmentManager::new());
        
        tokio::spawn(async move {
            while let Some(uri) = rx.recv().await {
                let plugin = HLSPlugin {
                    config: HLSConfig::default(),
                    state: Arc::new(RwLock::new(HLSState::default())),
                    segment_manager: segment_manager_clone.clone(),
                    cache: cache_clone.clone(),
                    prefetch_tx: tx_clone.clone(),  // 使用克隆的发送器
                };

                match plugin.handle_hls_request(&uri).await {
                    Ok(_) => debug!("Successfully prefetched: {}", uri),
                    Err(e) => warn!("Failed to prefetch {}: {}", uri, e),
                }
            }
        });

        Self {
            config: HLSConfig::default(),
            state: Arc::new(RwLock::new(HLSState::default())),
            segment_manager: Arc::new(SegmentManager::new()),
            cache,
            prefetch_tx: tx,  // 使用原始发送器
        }
    }

    /// 处理 HLS 请求
    pub async fn handle_hls_request(&self, uri: &str) -> Result<Vec<u8>, PluginError> {
        let cache_key = format!("hls:{}", uri);

        // 先尝试从缓存获取
        if let Ok((data, _)) = self.cache.get(&cache_key).await {
            return Ok(data);
        }

        // 缓存未命中，需要下载
        let bytes = self.download_segment(uri, "default").await?;
        let data = bytes.to_vec();

        // 如果是 m3u8 文件，解析并预下载
        if uri.ends_with(".m3u8") {
            self.handle_playlist(&data, uri).await?;
        }

        // 缓存响应
        let content_type = if uri.ends_with(".m3u8") {
            "application/vnd.apple.mpegurl"
        } else {
            "video/mp2t"
        };

        let metadata = CacheMetadata::new(content_type.into())
            .with_etag(format!("hls-{}", Uuid::new_v4()));

        // 克隆数据用于缓存，留原始数据用于返回
        let cache_data = data.clone();
        if let Err(e) = self.cache.store(cache_key, cache_data, metadata).await {
            warn!("Failed to cache response: {}", e);
        }

        Ok(data)
    }

    /// 处理播放列表
    async fn handle_playlist(&self, data: &[u8], uri: &str) -> Result<(), PluginError> {
        let playlist_str = String::from_utf8_lossy(data);
        
        // 尝试解析为主播放列表
        if let Ok((_, master_playlist)) = parse_master_playlist(playlist_str.as_bytes()) {
            self.handle_master_playlist(&master_playlist).await?;
        } else if let Ok((_, media_playlist)) = parse_media_playlist(playlist_str.as_bytes()) {
            self.handle_media_playlist(&media_playlist).await?;
        } else {
            warn!("Failed to parse playlist: {}", uri);
        }

        Ok(())
    }

    /// 处理主播放列表
    async fn handle_master_playlist(&self, playlist: &MasterPlaylist) -> Result<(), PluginError> {
        for variant in &playlist.variants {
            let uri = variant.uri.clone();
            self.prefetch_resource(&uri).await;
        }
        Ok(())
    }

    /// 处理媒体播放列表
    async fn handle_media_playlist(&self, playlist: &MediaPlaylist) -> Result<(), PluginError> {
        for segment in &playlist.segments {
            let uri = segment.uri.clone();
            self.prefetch_resource(&uri).await;
        }
        Ok(())
    }

    /// 预取资源
    async fn prefetch_resource(&self, uri: &str) {
        if let Err(e) = self.prefetch_tx.send(uri.to_string()).await {
            warn!("Failed to queue prefetch for {}: {}", uri, e);
        }
    }

    pub async fn get_active_stream_count(&self) -> usize {
        let state = self.state.read().await;
        state.active_streams.len()
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
        let mut state = self.state.write().await;
        let now = Instant::now();
        let ttl = Duration::from_secs(3600); // 1小时

        let expired: Vec<_> = state.active_streams
            .iter()
            .filter(|(_, s)| now.duration_since(s.last_access) > ttl)
            .map(|(id, _)| id.clone())
            .collect();

        for stream_id in expired {
            state.active_streams.remove(&stream_id);
            info!("Removed expired stream: {}", stream_id);
        }

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
        
        info!("Stream {} added successfully", stream_id);
        Ok(())
    }

    pub async fn update_bandwidth(&self, stream_id: &str, bandwidth: f64) -> Result<(), PluginError> {
        let mut state = self.state.write().await;
        if let Some(stream) = state.active_streams.get_mut(stream_id) {
            stream.bandwidth = bandwidth;
            stream.last_access = Instant::now();
            Ok(())
        } else {
            Err(PluginError::Hls("Stream not found".into()))
        }
    }

    #[instrument(skip(self))]
    async fn handle_segment_download(&self, url: &str, stream_id: &str) -> Result<Bytes, PluginError> {
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
        let client = hyper::Client::new();
        let uri = url.parse().map_err(|e| PluginError::Network(format!("Invalid URL: {}", e)))?;
        
        match client.get(uri).await {
            Ok(response) => {
                let bytes = hyper::body::to_bytes(response.into_body())
                    .await
                    .map_err(|e| PluginError::Network(e.to_string()))?;
                
                // 更新分片列表
                stream.segments.push(url.to_string());
                info!(
                    "Added new segment to stream {}. Total segments: {}",
                    stream_id,
                    stream.segments.len()
                );

                Ok(bytes)
            }
            Err(e) => {
                error!("Failed to download segment: {}", e);
                Err(PluginError::Network(e.to_string()))
            }
        }
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

    pub async fn download_segment(&self, url: &str, stream_id: &str) -> Result<Bytes, PluginError> {
        debug!("Starting segment download for stream {} from {}", stream_id, url);
        
        // 下载分片
        let bytes = self.handle_segment_download(url, stream_id).await?;
        let size = bytes.len();
        
        // 更新分片管理器
        self.segment_manager.add_segment(url.to_string(), size as u64).await;
        debug!(
            "Updated segment manager for stream {}. URL: {}, Size: {}",
            stream_id, url, size
        );
        
        Ok(bytes)  // 直接返回 Bytes，避免不必要的转换
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

// 为所有相关类型实现 Send + Sync
unsafe impl Send for HLSConfig {}
unsafe impl Sync for HLSConfig {}

unsafe impl Send for HLSState {}
unsafe impl Sync for HLSState {}

unsafe impl Send for StreamInfo {}
unsafe impl Sync for StreamInfo {}

// 为 HLSPlugin 实现 Send + Sync
unsafe impl Send for HLSPlugin {}
unsafe impl Sync for HLSPlugin {}

impl Clone for HLSPlugin {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            state: self.state.clone(),
            segment_manager: self.segment_manager.clone(),
            cache: self.cache.clone(),
            prefetch_tx: self.prefetch_tx.clone(),
        }
    }
}

// 为 HLSConfig 实现 Clone
impl Clone for HLSConfig {
    fn clone(&self) -> Self {
        Self {
            cache_dir: self.cache_dir.clone(),
            segment_duration: self.segment_duration,
            max_segments: self.max_segments,
            bandwidth_check_interval: self.bandwidth_check_interval,
        }
    }
} 