use std::sync::Arc;
use std::collections::HashMap;
use tokio::sync::{RwLock, mpsc};
use tracing::{debug, info, warn, error};
use async_trait::async_trait;
use m3u8_rs::{parse_master_playlist, parse_media_playlist};
use hyper::body::HttpBody;
use tokio::time::{timeout, Duration};
use hyper::client::HttpConnector;
use serde::{Serialize, Deserialize};
use std::path::{Path, PathBuf};

use crate::error::PluginError;
use crate::plugin::Plugin;
use crate::plugins::cache::{CacheManager, CacheMetadata};
use crate::proxy::MediaHandler;

#[derive(Debug)]
pub struct HLSPlugin {
    state: Arc<RwLock<HLSState>>,
    cache: Arc<CacheManager>,
    prefetch_tx: mpsc::Sender<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HLSState {
    segments: HashMap<String, u64>,  // URI -> total_size
}

impl HLSPlugin {
    pub fn new(cache_dir: String, cache: Arc<CacheManager>) -> Self {
        info!("Initializing HLSPlugin with cache directory: {}", cache_dir);
        
        // 确保 HLS 缓存目录存在
        tokio::task::block_in_place(|| {
            if let Err(e) = std::fs::create_dir_all(&cache_dir) {
                error!("Failed to create HLS cache directory: {}", e);
            } else {
                info!("HLS cache directory ready: {}", cache_dir);
            }
        });

        // 尝试加载持久化的状态
        let state = Self::load_state(&cache_dir).unwrap_or_else(|e| {
            warn!("Failed to load HLS state: {}, creating new state", e);
            HLSState {
                segments: HashMap::new(),
            }
        });

        info!("Loaded HLS state with {} segments", state.segments.len());

        let (tx, mut rx) = mpsc::channel(100);
        let tx_clone = tx.clone();
        let cache_clone = cache.clone();
        let state = Arc::new(RwLock::new(state));
        let state_clone = state.clone();
        let cache_dir = PathBuf::from(cache_dir);

        // 启动预取任务
        tokio::spawn(async move {
            debug!("Starting HLS prefetch task");
            while let Some(uri) = rx.recv().await {
                debug!("Received prefetch request for: {}", uri);
                let plugin = HLSPlugin {
                    state: state_clone.clone(),
                    cache: cache_clone.clone(),
                    prefetch_tx: tx_clone.clone(),
                };

                if let Err(e) = plugin.handle_hls_request(&uri).await {
                    warn!("Failed to prefetch {}: {}", uri, e);
                }
            }
        });

        // 启动状态保存任务
        let state_clone = state.clone();
        let cache_dir_clone = cache_dir.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(60)).await;
                if let Err(e) = Self::save_state(&cache_dir_clone, &state_clone).await {
                    error!("Failed to save HLS state: {}", e);
                }
            }
        });

        Self {
            state,
            cache,
            prefetch_tx: tx,
        }
    }

    async fn save_state(cache_dir: &Path, state: &RwLock<HLSState>) -> Result<(), PluginError> {
        let state_path = cache_dir.join("hls_state.json");
        let temp_path = state_path.with_extension("tmp");
        
        let state_guard = state.read().await;
        let state_json = serde_json::to_string_pretty(&*state_guard)
            .map_err(|e| PluginError::Storage(format!("Failed to serialize HLS state: {}", e)))?;
        
        tokio::fs::write(&temp_path, state_json).await
            .map_err(|e| PluginError::Storage(format!("Failed to write HLS state: {}", e)))?;
        
        tokio::fs::rename(&temp_path, &state_path).await
            .map_err(|e| PluginError::Storage(format!("Failed to save HLS state: {}", e)))?;
        
        debug!("HLS state saved successfully");
        Ok(())
    }

    fn load_state(cache_dir: &str) -> Result<HLSState, PluginError> {
        let state_path = Path::new(cache_dir).join("hls_state.json");
        
        if !state_path.exists() {
            debug!("No existing state file found, starting fresh");
            return Ok(HLSState {
                segments: HashMap::new(),
            });
        }

        let state_json = std::fs::read_to_string(&state_path)
            .map_err(|e| PluginError::Storage(format!("Failed to read HLS state: {}", e)))?;
        
        let state: HLSState = serde_json::from_str(&state_json)
            .map_err(|e| PluginError::Storage(format!("Failed to parse HLS state: {}", e)))?;
        
        Ok(state)
    }

    async fn handle_hls_request(&self, uri: &str) -> Result<Vec<u8>, PluginError> {
        let cache_key = format!("hls:{}", uri);
        info!("Processing HLS request: {}", uri);

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
        state.segments.insert(uri.to_string(), total_size as u64);
        info!("Download completed for {}, total size: {} bytes", uri, total_size);

        if uri.ends_with(".m3u8") {
            self.handle_playlist(&all_data, uri).await?;
        }

        Ok(all_data)
    }

    async fn handle_playlist(&self, data: &[u8], uri: &str) -> Result<(), PluginError> {
        let playlist_str = String::from_utf8_lossy(data);
        info!("Processing playlist: {}", uri);
        
        if let Ok((_, master_playlist)) = parse_master_playlist(playlist_str.as_bytes()) {
            info!("Found master playlist with {} variants", master_playlist.variants.len());
            for variant in &master_playlist.variants {
                info!("Prefetching variant: {}", variant.uri);
                self.prefetch_tx.send(variant.uri.clone()).await
                    .map_err(|e| PluginError::Plugin(format!("Failed to queue prefetch: {}", e)))?;
            }
        } else if let Ok((_, media_playlist)) = parse_media_playlist(playlist_str.as_bytes()) {
            info!("Found media playlist with {} segments", media_playlist.segments.len());
            for segment in &media_playlist.segments {
                info!("Prefetching segment: {}", segment.uri);
                self.prefetch_tx.send(segment.uri.clone()).await
                    .map_err(|e| PluginError::Plugin(format!("Failed to queue prefetch: {}", e)))?;
            }
        } else {
            warn!("Invalid playlist format: {}", uri);
        }

        Ok(())
    }
}

#[async_trait]
impl Plugin for HLSPlugin {
    fn name(&self) -> &str { "hls" }
    fn version(&self) -> &str { "1.0.0" }
    async fn init(&self) -> Result<(), PluginError> { Ok(()) }
    async fn cleanup(&self) -> Result<(), PluginError> { Ok(()) }
}

#[async_trait]
impl MediaHandler for HLSPlugin {
    async fn handle_request(&self, uri: &str) -> Result<Vec<u8>, PluginError> {
        self.handle_hls_request(uri).await
    }

    fn can_handle(&self, uri: &str) -> bool {
        uri.ends_with(".m3u8") || uri.ends_with(".ts")
    }

    async fn stream_request(&self, uri: &str, mut sender: hyper::body::Sender) -> Result<(), PluginError> {
        let cache_key = format!("hls:{}", uri);
        info!("Processing HLS stream request: {}", uri);

        // 检查缓存
        info!("Checking cache for key: {}", cache_key);
        let cache_result = self.cache.get(&cache_key).await;
        
        match cache_result {
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
                return Ok(());
            }
            Err(e) => {
                info!("Cache miss for {}: {}", uri, e);
            }
        }

        info!("Cache miss for {}, downloading...", uri);
        let client: hyper::Client<HttpConnector> = hyper::Client::builder()
            .pool_idle_timeout(Duration::from_secs(30))
            .build_http();

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

        let content_type = if uri.ends_with(".m3u8") { 
            "application/vnd.apple.mpegurl" 
        } else { 
            "video/mp2t" 
        };
        let metadata = CacheMetadata::new(content_type.into());
        self.cache.store_stream(cache_key.clone(), metadata).await?;

        let mut playlist_data = Vec::new();
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

                            // 克隆数据用于缓存和播放列表处理
                            let chunk_copy = chunk.clone();
                            
                            // 写入缓存
                            if let Err(e) = self.cache.append_chunk(&cache_key, &chunk_copy).await {
                                error!("Failed to cache chunk: {}", e);
                                // 继续发送数据，即使缓存失败
                            }
                            
                            // 如果是播放列表，收集数据
                            if uri.ends_with(".m3u8") {
                                playlist_data.extend_from_slice(&chunk_copy);
                            }
                            
                            // 发送给客户端
                            if let Err(e) = sender.send_data(chunk).await {
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

        // 如果是播放列表，处理预取
        if uri.ends_with(".m3u8") {
            self.handle_playlist(&playlist_data, uri).await?;
        }

        let mut state = self.state.write().await;
        state.segments.insert(uri.to_string(), total_size as u64);
        info!("Stream completed for {}, total size: {} bytes, chunks: {}", 
            uri, total_size, chunk_count);

        Ok(())
    }
}

unsafe impl Send for HLSPlugin {}
unsafe impl Sync for HLSPlugin {}

impl Clone for HLSPlugin {
    fn clone(&self) -> Self {
        Self {
            state: self.state.clone(),
            cache: self.cache.clone(),
            prefetch_tx: self.prefetch_tx.clone(),
        }
    }
} 