mod types;
mod adaptive;
mod playlist;
mod segment;
mod storage;
mod utils;

use std::sync::Arc;
use tokio::sync::{RwLock, mpsc};
use tracing::{debug, info, warn, error};
use async_trait::async_trait;

use crate::error::PluginError;
use crate::plugin::Plugin;
use crate::plugins::cache::CacheManager;
use crate::proxy::MediaHandler;

pub use self::types::{PlaylistType, PlaylistInfo, Variant, Segment};
use self::storage::StorageManager;
use self::playlist::PlaylistManager;
use self::segment::SegmentManager;
use self::adaptive::AdaptiveStreamManager;

#[derive(Debug)]
pub struct HLSPlugin {
    storage: Arc<StorageManager>,
    playlist: Arc<PlaylistManager>,
    segment: Arc<SegmentManager>,
    adaptive: Arc<AdaptiveStreamManager>,
    cache: Arc<CacheManager>,
}

impl HLSPlugin {
    pub fn new(cache: Arc<CacheManager>) -> Self {
        let storage = Arc::new(StorageManager::new("cache/hls"));
        let segment = Arc::new(SegmentManager::new());
        let playlist = Arc::new(PlaylistManager::new(cache.clone(), segment.clone()));
        let adaptive = Arc::new(AdaptiveStreamManager::new());

        // 创建缓存目录
        tokio::task::block_in_place(|| {
            if let Err(e) = std::fs::create_dir_all("cache/hls") {
                error!("Failed to create HLS cache directory: {}", e);
            }
        });

        Self {
            storage,
            playlist,
            segment,
            adaptive,
            cache,
        }
    }

    /// 预取变体流
    async fn prefetch_variant(&self, variant: &Variant) {
        debug!("Prefetching variant playlist: {}", variant.url);
        if let Err(e) = self.playlist.prefetch_variant(variant).await {
            warn!("Failed to prefetch variant {}: {}", variant.url, e);
        }
    }

    /// 预取分片
    async fn prefetch_segments(&self, segments: &[Segment], count: usize) {
        if let Err(e) = self.playlist.prefetch_segments(segments, count).await {
            warn!("Failed to prefetch segments: {}", e);
        }
    }

    /// 更新带宽统计
    async fn update_bandwidth(&self, uri: &str, bytes: u64, duration: std::time::Duration) {
        if let Ok(playlist_url) = self.playlist.get_playlist_url(uri).await {
            let bps = (bytes as f64 * 8.0) / duration.as_secs_f64();
            self.adaptive.update_bandwidth(&playlist_url, bps).await;
            debug!("Updated bandwidth for {}: {:.2} Mbps", uri, bps / 1_000_000.0);
        }
    }

    /// 获取当前带宽
    pub async fn get_bandwidth(&self, uri: &str) -> f64 {
        self.adaptive.get_bandwidth(uri).await
    }
}

#[async_trait]
impl Plugin for HLSPlugin {
    fn name(&self) -> &str { "hls" }
    fn version(&self) -> &str { "1.0.0" }
    
    async fn init(&self) -> Result<(), PluginError> {
        self.storage.init().await?;
        
        // 创建定期清理任务
        let playlist = self.playlist.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(300)).await; // 每5分钟
                if let Err(e) = playlist.cleanup().await {
                    error!("Failed to cleanup playlists: {}", e);
                }
            }
        });

        Ok(())
    }
    
    async fn cleanup(&self) -> Result<(), PluginError> {
        self.storage.cleanup().await?;
        self.playlist.cleanup().await?;
        Ok(())
    }

    async fn health_check(&self) -> Result<bool, PluginError> {
        // 获取统计信息
        let stats = self.playlist.get_stats().await;
        info!("HLS plugin stats: {:?}", stats);
        Ok(true)
    }
}

#[async_trait]
impl MediaHandler for HLSPlugin {
    async fn handle_request(&self, uri: &str) -> Result<Vec<u8>, PluginError> {
        if uri.ends_with(".m3u8") {
            // 处理播放列表请求
            let content = self.playlist.handle_playlist(uri).await?;
            
            // 如果是主播放列表，根据带宽选择合适的变体流
            if let Some(info) = self.playlist.get_playlist_info(uri).await {
                if matches!(info.playlist_type, PlaylistType::Master) {
                    if let Some(variant) = self.adaptive.get_optimal_variant(&info.url, &info.variants).await {
                        debug!("Selected variant: {} ({} bps)", variant.url, variant.bandwidth);
                        // 预取选中的变体流
                        if let Err(e) = self.playlist.prefetch_variant(variant).await {
                            warn!("Failed to prefetch variant {}: {}", variant.url, e);
                        }
                    }
                } else {
                    // 对于媒体播放列表，预取前几个分片
                    let prefetch_count = 3; // 默认预取3个分片
                    if !info.segments.is_empty() {
                        debug!("Prefetching first {} segments", prefetch_count);
                        if let Err(e) = self.playlist.prefetch_segments(&info.segments, prefetch_count).await {
                            warn!("Failed to prefetch segments: {}", e);
                        }
                    }
                }
            }
            
            Ok(content)
        } else {
            self.segment.handle_segment(uri).await
        }
    }

    async fn stream_request(&self, uri: &str, sender: hyper::body::Sender) -> Result<(), PluginError> {
        if uri.ends_with(".m3u8") {
            self.playlist.stream_playlist(uri, sender).await
        } else {
            self.segment.stream_segment(uri, sender).await
        }
    }

    fn can_handle(&self, uri: &str) -> bool {
        uri.ends_with(".m3u8") || uri.ends_with(".ts")
    }
} 