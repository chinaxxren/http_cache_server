use std::sync::Arc;
use std::collections::HashMap;
use tokio::sync::{RwLock, mpsc};
use tracing::{debug, info, warn};
use async_trait::async_trait;

use crate::error::PluginError;
use crate::plugin::Plugin;
use crate::plugins::cache::CacheManager;
use crate::proxy::MediaHandler;

#[derive(Debug)]
pub struct MP4Plugin {
    state: Arc<RwLock<MP4State>>,
    cache: Arc<CacheManager>,
    prefetch_tx: mpsc::Sender<String>,
}

#[derive(Debug)]
struct MP4State {
    downloads: HashMap<String, u64>,  // URI -> total_size
}

impl Default for MP4State {
    fn default() -> Self {
        Self {
            downloads: HashMap::new(),
        }
    }
}

impl MP4Plugin {
    pub fn new(cache: Arc<CacheManager>) -> Self {
        info!("Initializing MP4Plugin");
        let (tx, mut rx) = mpsc::channel(100);
        let tx_clone = tx.clone();
        let cache_clone = cache.clone();

        tokio::spawn(async move {
            debug!("Starting MP4 prefetch task");
            while let Some(uri) = rx.recv().await {
                debug!("Received prefetch request for: {}", uri);
                let plugin = MP4Plugin {
                    state: Arc::new(RwLock::new(MP4State::default())),
                    cache: cache_clone.clone(),
                    prefetch_tx: tx_clone.clone(),
                };

                if let Err(e) = plugin.handle_mp4_request(&uri).await {
                    warn!("Failed to prefetch {}: {}", uri, e);
                }
            }
        });

        Self {
            state: Arc::new(RwLock::new(MP4State::default())),
            cache,
            prefetch_tx: tx,
        }
    }

    async fn handle_mp4_request(&self, uri: &str) -> Result<Vec<u8>, PluginError> {
        let cache_key = format!("mp4:{}", uri);
        info!("Processing MP4 request: {}", uri);

        if let Ok((data, _)) = self.cache.get(&cache_key).await {
            info!("Cache hit for {}, size: {} bytes", uri, data.len());
            return Ok(data);
        }

        info!("Cache miss for {}, downloading...", uri);
        let client = hyper::Client::new();
        let uri_parsed = uri.parse::<hyper::Uri>()
            .map_err(|e| PluginError::Network(format!("Invalid URL: {}", e)))?;
        
        let mut response = client.get(uri_parsed).await
            .map_err(|e| PluginError::Network(e.to_string()))?;

        let status = response.status();
        info!("Download started for {}, status: {}", uri, status);

        let mut all_data = Vec::new();
        let mut total_size = 0;
        
        while let Some(chunk) = hyper::body::HttpBody::data(&mut response.body_mut()).await {
            let chunk = chunk.map_err(|e| PluginError::Network(e.to_string()))?;
            total_size += chunk.len();
            debug!("Received chunk: {} bytes, total: {} bytes", chunk.len(), total_size);
            self.cache.append_chunk(&cache_key, &chunk).await?;
            all_data.extend_from_slice(&chunk);
        }

        // 更新下载统计
        let mut state = self.state.write().await;
        state.downloads.insert(uri.to_string(), total_size as u64);
        info!("Download completed for {}, total size: {} bytes", uri, total_size);

        Ok(all_data)
    }
}

#[async_trait]
impl Plugin for MP4Plugin {
    fn name(&self) -> &str { "mp4" }
    fn version(&self) -> &str { "1.0.0" }
    async fn init(&self) -> Result<(), PluginError> { Ok(()) }
    async fn cleanup(&self) -> Result<(), PluginError> { Ok(()) }
}

#[async_trait]
impl MediaHandler for MP4Plugin {
    async fn handle_request(&self, uri: &str) -> Result<Vec<u8>, PluginError> {
        self.handle_mp4_request(uri).await
    }

    fn can_handle(&self, uri: &str) -> bool {
        uri.ends_with(".mp4")
    }
}

unsafe impl Send for MP4Plugin {}
unsafe impl Sync for MP4Plugin {}

impl Clone for MP4Plugin {
    fn clone(&self) -> Self {
        Self {
            state: self.state.clone(),
            cache: self.cache.clone(),
            prefetch_tx: self.prefetch_tx.clone(),
        }
    }
} 