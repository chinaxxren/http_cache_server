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

    async fn create_client(&self, uri: &str) -> hyper::Client<HttpsConnector<HttpConnector>> {
        let https = HttpsConnector::new();
        hyper::Client::builder()
            .pool_idle_timeout(Duration::from_secs(30))
            .build::<_, hyper::Body>(https)
    }

    async fn handle_mp4_request(&self, uri: &str) -> Result<Vec<u8>, PluginError> {
        let cache_key = format!("mp4:{}", uri);
        info!("Processing MP4 request: {}", uri);

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

    async fn stream_request(&self, uri: &str, mut sender: hyper::body::Sender) -> Result<(), PluginError> {
        let cache_key = format!("mp4:{}", uri);
        info!("Processing MP4 stream request: {}", uri);

        // 检查缓存
        info!("Checking cache for key: {}", cache_key);
        let cache_result = self.cache.get(&cache_key).await;
        
        match cache_result {
            Ok((data, _)) => {
                info!("Cache hit for {}, streaming {} bytes", uri, data.len());
                // 分块发送缓存数据，避免一次性加载大文件到内存
                const CHUNK_SIZE: usize = 1024 * 1024; // 1MB chunks
                let mut offset = 0;
                while offset < data.len() {
                    let end = (offset + CHUNK_SIZE).min(data.len());
                    let chunk = bytes::Bytes::copy_from_slice(&data[offset..end]);
                    sender.send_data(chunk).await
                        .map_err(|e| PluginError::Network(format!("Failed to send cached chunk: {}", e)))?;
                    offset = end;
                }
                return Ok(());
            }
            Err(e) => {
                info!("Cache miss for {}: {}", uri, e);
            }
        }

        info!("Starting download for {}", uri);
        let client = self.create_client(uri).await;

        let uri_parsed = uri.parse::<hyper::Uri>()
            .map_err(|e| PluginError::Network(format!("Invalid URL: {}", e)))?;
        
        let mut response = match timeout(Duration::from_secs(30), client.get(uri_parsed)).await {
            Ok(result) => result.map_err(|e| PluginError::Network(e.to_string()))?,
            Err(_) => return Err(PluginError::Network("Request timeout".into())),
        };

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

        // 创建缓存流
        info!("Creating cache stream for {}", uri);
        let metadata = CacheMetadata::new("video/mp4".into());
        match self.cache.store_stream(cache_key.clone(), metadata).await {
            Ok(_) => info!("Cache stream created successfully"),
            Err(e) => warn!("Failed to create cache stream: {}", e),
        }

        let mut total_size = 0;
        let mut chunk_count = 0;
        let mut retry_count = 0;
        const MAX_RETRIES: u32 = 3;
        
        'download: loop {
            match timeout(Duration::from_secs(30), response.body_mut().data()).await {
                Ok(Some(chunk_result)) => {
                    match chunk_result {
                        Ok(chunk) => {
                            chunk_count += 1;
                            let chunk_len = chunk.len();
                            total_size += chunk_len;
                            
                            debug!("Processing chunk {}: {} bytes, total: {} bytes", 
                                chunk_count, chunk_len, total_size);

                            // 写入缓存
                            if let Err(e) = self.cache.append_chunk(&cache_key, &chunk).await {
                                warn!("Failed to cache chunk: {}", e);
                                // 继续发送数据，即使缓存失败
                            }
                            
                            // 发送给客户端
                            if let Err(e) = sender.send_data(chunk.into()).await {
                                error!("Failed to send chunk to client: {}", e);
                                return Err(PluginError::Network(format!("Failed to send chunk: {}", e)));
                            }

                            // 每处理 100 个块输出一次进度
                            if chunk_count % 100 == 0 {
                                info!("Progress: {} chunks, {} bytes transferred ({:.2}%)", 
                                    chunk_count, 
                                    total_size,
                                    content_length.map_or(0.0, |len| (total_size as f64 / len as f64) * 100.0)
                                );
                            }

                            retry_count = 0;  // 重置重试计数
                        }
                        Err(e) => {
                            error!("Failed to read chunk: {}", e);
                            if retry_count < MAX_RETRIES {
                                retry_count += 1;
                                warn!("Chunk error, retrying ({}/{})", retry_count, MAX_RETRIES);
                                continue;
                            }
                            return Err(PluginError::Network(format!("Failed to read chunk: {}", e)));
                        }
                    }
                }
                Ok(None) => break 'download,  // 数据流结束
                Err(_) => {
                    if retry_count < MAX_RETRIES {
                        retry_count += 1;
                        warn!("Chunk timeout, retrying ({}/{})", retry_count, MAX_RETRIES);
                        continue;
                    }
                    return Err(PluginError::Network("Chunk timeout".into()));
                }
            }
        }

        let mut state = self.state.write().await;
        state.downloads.insert(uri.to_string(), total_size as u64);
        info!("Stream completed for {}, total size: {} bytes, chunks: {}", 
            uri, total_size, chunk_count);

        Ok(())
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