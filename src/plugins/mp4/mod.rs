use std::sync::Arc;
use std::collections::HashMap;
use tokio::sync::{RwLock, mpsc};
use tracing::{debug, info, warn, error};
use async_trait::async_trait;
use hyper::body::HttpBody;
use tokio::time::{timeout, Duration};
use hyper::client::HttpConnector;
use hyper_tls::HttpsConnector;

use crate::error::PluginError;
use crate::plugin::Plugin;
use crate::plugins::cache::{CacheManager, CacheMetadata};
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
        
        // 确保 MP4 缓存目录存在
        let cache_dir = "./cache/mp4";
        tokio::task::block_in_place(|| {
            if let Err(e) = std::fs::create_dir_all(cache_dir) {
                error!("Failed to create MP4 cache directory: {}", e);
            } else {
                info!("MP4 cache directory ready: {}", cache_dir);
            }
        });

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

    pub fn hash_url(&self, uri: &str) -> String {
        let cache_key = format!("mp4:{}", self.hash_url(uri));
        cache_key
    }

    async fn create_client(&self, uri: &str) -> hyper::Client<HttpsConnector<HttpConnector>> {
        let https = HttpsConnector::new();
        hyper::Client::builder()
            .pool_idle_timeout(Duration::from_secs(30))
            .build::<_, hyper::Body>(https)
    }

    async fn handle_mp4_request(&self, uri: &str) -> Result<Vec<u8>, PluginError> {
        let cache_key = self.hash_url(uri);
        info!("Processing MP4 request:{} {}", cache_key, uri);
 
        // 检查缓存
        match self.cache.get(&cache_key).await {
            Ok((data, _)) => {
                info!("Cache hit for {}, size: {} bytes", uri, data.len());
                return Ok(data);
            }
            Err(e) => {
                debug!("Cache miss for {}: {}", uri, e);
            }
        }

        info!("Cache miss for {}, downloading...", uri);
        let client = self.create_client(uri).await;

        let uri_parsed = uri.parse::<hyper::Uri>()
            .map_err(|e| PluginError::Network(format!("Invalid URL: {}", e)))?;
        
        let mut response = client.get(uri_parsed).await
            .map_err(|e| PluginError::Network(e.to_string()))?;

        let status = response.status();
        info!("Download started for {}, status: {}", uri, status);

        if !status.is_success() {
            return Err(PluginError::Network(format!("Server returned status: {}", status)));
        }

        let metadata = CacheMetadata::new("video/mp4".into());
        self.cache.store_stream(cache_key.clone(), metadata).await?;

        let mut all_data = Vec::new();
        let mut total_size = 0;
        
        while let Some(chunk) = hyper::body::HttpBody::data(&mut response.body_mut()).await {
            let chunk = chunk.map_err(|e| PluginError::Network(e.to_string()))?;
            total_size += chunk.len();
            debug!("Received chunk: {} bytes, total: {} bytes", chunk.len(), total_size);
            
            self.cache.append_chunk(&cache_key, &chunk).await?;
            
            all_data.extend_from_slice(&chunk);
        }

        let mut state = self.state.write().await;
        state.downloads.insert(uri.to_string(), total_size as u64);
        info!("Download completed for {}, total size: {} bytes", uri, total_size);

        Ok(all_data)
    }

    async fn download_and_cache(&self, uri: &str, cache_key: &str) -> Result<Vec<u8>, PluginError> {
        let client = self.create_client(uri).await;
        let uri_parsed = uri.parse::<hyper::Uri>()
            .map_err(|e| PluginError::Network(format!("Invalid URL: {}", e)))?;
        
        let mut response = client.get(uri_parsed).await
            .map_err(|e| PluginError::Network(e.to_string()))?;

        let status = response.status();
        info!("Download started for {}, status: {}", uri, status);

        if !status.is_success() {
            return Err(PluginError::Network(format!("Server returned status: {}", status)));
        }

        let metadata = CacheMetadata::new("video/mp4".into());
        self.cache.store_stream(cache_key.to_string(), metadata).await?;

        let mut all_data = Vec::new();
        let mut total_size = 0;
        
        while let Some(chunk) = hyper::body::HttpBody::data(&mut response.body_mut()).await {
            let chunk = chunk.map_err(|e| PluginError::Network(e.to_string()))?;
            total_size += chunk.len();
            debug!("Received chunk: {} bytes, total: {} bytes", chunk.len(), total_size);
            
            self.cache.append_chunk(cache_key, &chunk).await?;
            all_data.extend_from_slice(&chunk);
        }

        let mut state = self.state.write().await;
        state.downloads.insert(uri.to_string(), total_size as u64);
        info!("Download completed for {}, total size: {} bytes", uri, total_size);

        Ok(all_data)
    }

    async fn stream_download(&self, uri: &str, cache_key: &str, mut sender: hyper::body::Sender) -> Result<(), PluginError> {
        let client = self.create_client(uri).await;
        let uri_parsed = uri.parse::<hyper::Uri>()
            .map_err(|e| PluginError::Network(format!("Invalid URL: {}", e)))?;
        
        let mut response = client.get(uri_parsed).await
            .map_err(|e| PluginError::Network(e.to_string()))?;

        let status = response.status();
        let content_length = response.headers()
            .get(hyper::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());

        info!("Download started for {}, status: {}, content length: {:?}", 
            uri, status, content_length);

        if !status.is_success() {
            return Err(PluginError::Network(format!("Server returned status: {}", status)));
        }

        let metadata = CacheMetadata::new("video/mp4".into());
        self.cache.store_stream(cache_key.to_string(), metadata).await?;

        let mut total_size = 0;
        let mut chunk_count = 0;

        while let Some(chunk_result) = response.body_mut().data().await {
            let chunk = chunk_result.map_err(|e| PluginError::Network(e.to_string()))?;
            chunk_count += 1;
            total_size += chunk.len();
            
            debug!("Processing chunk {}: {} bytes, total: {} bytes", 
                chunk_count, chunk.len(), total_size);

            self.cache.append_chunk(cache_key, &chunk).await?;
            sender.send_data(chunk.into()).await
                .map_err(|e| PluginError::Network(format!("Failed to send chunk: {}", e)))?;

            if chunk_count % 100 == 0 {
                info!("Progress: {} chunks, {} bytes transferred ({:.2}%)", 
                    chunk_count, 
                    total_size,
                    content_length.map_or(0.0, |len| (total_size as f64 / len as f64) * 100.0)
                );
            }
        }

        let mut state = self.state.write().await;
        state.downloads.insert(uri.to_string(), total_size as u64);
        info!("Stream completed for {}, total size: {} bytes, chunks: {}", 
            uri, total_size, chunk_count);

        Ok(())
    }
}

#[async_trait]
impl Plugin for MP4Plugin {
    fn name(&self) -> &str { "mp4" }
    fn version(&self) -> &str { "1.0.0" }
    
    async fn init(&self) -> Result<(), PluginError> {
        Ok(())
    }
    
    async fn cleanup(&self) -> Result<(), PluginError> {
        Ok(())
    }

    async fn health_check(&self) -> Result<bool, PluginError> {
        let state = self.state.read().await;
        info!("MP4 plugin stats:");
        info!("  - Active downloads: {}", state.downloads.len());
        Ok(true)
    }
}

#[async_trait]
impl MediaHandler for MP4Plugin {
    async fn handle_request(&self, uri: &str) -> Result<Vec<u8>, PluginError> {
        let cache_key = self.hash_url(uri);
        info!("Processing MP4 request: {}", uri);

        // 检查缓存
        match self.cache.get(&cache_key).await {
            Ok((data, _)) => {
                info!("Cache hit for {}, size: {} bytes", uri, data.len());
                Ok(data)
            }
            Err(e) => {
                debug!("Cache miss for {}: {:?}", uri, e);
                self.download_and_cache(uri, &cache_key).await
            }
        }
    }

    fn can_handle(&self, uri: &str) -> bool {
        uri.ends_with(".mp4")
    }

    async fn stream_request(&self, uri: &str, mut sender: hyper::body::Sender) -> Result<(), PluginError> {
        let cache_key = self.hash_url(uri);
        info!("Processing MP4 stream request:{} {}", cache_key, uri);

        // 检查缓存
        match self.cache.get(&cache_key).await {
            Ok((data, _)) => {
                info!("Cache hit for {}, streaming {} bytes", uri, data.len());
                // 分块发送缓存数据
                const CHUNK_SIZE: usize = 1024 * 1024; // 1MB chunks
                let mut offset = 0;
                while offset < data.len() {
                    let end = (offset + CHUNK_SIZE).min(data.len());
                    let chunk = bytes::Bytes::copy_from_slice(&data[offset..end]);
                    sender.send_data(chunk).await
                        .map_err(|e| PluginError::Network(format!("Failed to send cached chunk: {}", e)))?;
                    offset = end;
                }
                Ok(())
            }
            Err(e) => {
                debug!("Cache miss for {}: {:?}", uri, e);
                self.stream_download(uri, &cache_key, sender).await
            }
        }
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