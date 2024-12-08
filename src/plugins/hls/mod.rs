use std::sync::Arc;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use tokio::sync::{RwLock, mpsc};
use tracing::{info, warn, error, debug};
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

#[derive(Debug, Default, Clone)]
struct HLSConfig {
    cache_dir: String,
    max_segments: usize,
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
}

impl HLSPlugin {
    pub fn new(cache_dir: String, cache: Arc<CacheManager>) -> Self {
        info!("Initializing HLSPlugin with cache directory: {}", cache_dir);
        
        let (tx, mut rx) = mpsc::channel(100);
        let tx_clone = tx.clone();
        
        let cache_clone = cache.clone();
        let segment_manager = Arc::new(SegmentManager::new());
        
        tokio::spawn(async move {
            while let Some(uri) = rx.recv().await {
                let plugin = HLSPlugin {
                    config: HLSConfig::default(),
                    state: Arc::new(RwLock::new(HLSState::default())),
                    segment_manager: segment_manager.clone(),
                    cache: cache_clone.clone(),
                    prefetch_tx: tx_clone.clone(),
                };

                if let Err(e) = plugin.handle_hls_request(&uri).await {
                    warn!("Failed to prefetch {}: {}", uri, e);
                }
            }
        });

        Self {
            config: HLSConfig::default(),
            state: Arc::new(RwLock::new(HLSState::default())),
            segment_manager,
            cache,
            prefetch_tx: tx,
        }
    }

    pub async fn handle_hls_request(&self, uri: &str) -> Result<Vec<u8>, PluginError> {
        let cache_key = format!("hls:{}", uri);

        if let Ok((data, _)) = self.cache.get(&cache_key).await {
            return Ok(data);
        }

        let client = hyper::Client::new();
        let uri_parsed = uri.parse::<hyper::Uri>()
            .map_err(|e| PluginError::Network(format!("Invalid URL: {}", e)))?;
        
        let mut response = client.get(uri_parsed).await
            .map_err(|e| PluginError::Network(e.to_string()))?;
        
        let mut all_data = Vec::new();
        
        while let Some(chunk) = hyper::body::HttpBody::data(&mut response.body_mut()).await {
            let chunk = chunk.map_err(|e| PluginError::Network(e.to_string()))?;
            self.cache.append_chunk(&cache_key, &chunk).await?;
            all_data.extend_from_slice(&chunk);
        }

        if uri.ends_with(".m3u8") {
            self.handle_playlist(&all_data, uri).await?;
        }

        Ok(all_data)
    }

    async fn handle_playlist(&self, data: &[u8], uri: &str) -> Result<(), PluginError> {
        let playlist_str = String::from_utf8_lossy(data);
        
        if let Ok((_, master_playlist)) = parse_master_playlist(playlist_str.as_bytes()) {
            for variant in &master_playlist.variants {
                self.prefetch_resource(&variant.uri).await;
            }
        } else if let Ok((_, media_playlist)) = parse_media_playlist(playlist_str.as_bytes()) {
            for segment in &media_playlist.segments {
                self.prefetch_resource(&segment.uri).await;
            }
        }

        Ok(())
    }

    async fn prefetch_resource(&self, uri: &str) {
        if let Err(e) = self.prefetch_tx.send(uri.to_string()).await {
            warn!("Failed to queue prefetch for {}: {}", uri, e);
        }
    }
}

#[async_trait]
impl Plugin for HLSPlugin {
    fn name(&self) -> &str { "hls" }
    fn version(&self) -> &str { "1.0.0" }
    async fn init(&self) -> Result<(), PluginError> { Ok(()) }
    async fn cleanup(&self) -> Result<(), PluginError> { Ok(()) }
    async fn health_check(&self) -> Result<bool, PluginError> { Ok(true) }
}

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