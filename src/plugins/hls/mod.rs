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
use hyper_tls;
use url;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

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
        
        // 使用绝对路径
        let cache_path = std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(&cache_dir);
            
        info!("Using absolute cache path: {:?}", cache_path);
        
        // 确保 HLS 缓存目录存在
        tokio::task::block_in_place(|| {
            if let Err(e) = std::fs::create_dir_all(&cache_path) {
                error!("Failed to create HLS cache directory: {}", e);
            } else {
                info!("HLS cache directory ready: {:?}", cache_path);
            }
        });

        // 尝试加载持久化的状态
        let state = Self::load_state(&cache_path).unwrap_or_else(|e| {
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
        let cache_dir_path = PathBuf::from(&cache_dir);

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

        // 启动状态保存任务，每10秒保存一次
        let state_clone = state.clone();
        let cache_dir_clone = cache_dir_path.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(10)).await;
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

    fn load_state(cache_dir: &Path) -> Result<HLSState, PluginError> {
        let state_path = cache_dir.join("hls_state.json");
        info!("Loading HLS state from: {:?}", state_path);
        
        if !state_path.exists() {
            info!("No existing state file found at {:?}, starting fresh", state_path);
            return Ok(HLSState {
                segments: HashMap::new(),
            });
        }

        match std::fs::read_to_string(&state_path) {
            Ok(content) => {
                info!("Successfully read state file, content length: {} bytes", content.len());
                match serde_json::from_str(&content) {
                    Ok(state) => {
                        info!("Successfully parsed state file");
                        Ok(state)
                    }
                    Err(e) => {
                        error!("Failed to parse state file: {}", e);
                        Err(PluginError::Storage(format!("Failed to parse state file: {}", e)))
                    }
                }
            }
            Err(e) => {
                error!("Failed to read state file: {}", e);
                Err(PluginError::Storage(format!("Failed to read state file: {}", e)))
            }
        }
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
        let client = self.create_client(uri).await?;

        let uri_parsed = uri.parse::<hyper::Uri>()
            .map_err(|e| {
                error!("Failed to parse URL {}: {}", uri, e);
                PluginError::Network(format!("Invalid URL: {}", e))
            })?;

        info!("Sending request to: {}", uri_parsed);
        let mut response = client.get(uri_parsed).await
            .map_err(|e| {
                error!("Request failed for {}: {}", uri, e);
                PluginError::Network(e.to_string())
            })?;

        let status = response.status();
        info!("Response status for {}: {}", uri, status);

        // 打印响应头
        debug!("Response headers:");
        for (name, value) in response.headers() {
            debug!("  {}: {:?}", name, value);
        }

        if !status.is_success() {
            return Err(PluginError::Network(format!("Server returned status: {}", status)));
        }

        // 确保目录存在
        let file_path = self.cache.get_path(&cache_key);
        if let Some(parent) = file_path.parent() {
            tokio::fs::create_dir_all(parent).await
                .map_err(|e| PluginError::Storage(format!("Failed to create directory: {}", e)))?;
        }

        let content_type = response.headers()
            .get(hyper::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("application/vnd.apple.mpegurl");

        let metadata = CacheMetadata::new(content_type.into());
        self.cache.store_stream(cache_key.clone(), metadata).await?;

        let mut all_data = Vec::new();
        let mut total_size = 0;
        
        while let Some(chunk) = hyper::body::HttpBody::data(&mut response.body_mut()).await {
            let chunk = chunk.map_err(|e| {
                error!("Failed to read chunk from {}: {}", uri, e);
                PluginError::Network(e.to_string())
            })?;
            
            total_size += chunk.len();
            debug!("Received chunk: {} bytes, total: {} bytes", chunk.len(), total_size);
            
            // 先保存数据
            all_data.extend_from_slice(&chunk);
            
            // 再写入缓存
            if let Err(e) = self.cache.append_chunk(&cache_key, &chunk).await {
                error!("Failed to cache chunk for {}: {}", uri, e);
                // 继续下载，即使缓存失败
            }
        }

        info!("Download completed for {}, total size: {} bytes", uri, total_size);

        // 如果是播放列表，尝试解析并预取
        if uri.ends_with(".m3u8") {
            let playlist_str = String::from_utf8_lossy(&all_data);
            debug!("Playlist content:\n{}", playlist_str);
            
            // 尝试解析主播放列表或媒体播放列表
            if let Ok((_, master_playlist)) = parse_master_playlist(playlist_str.as_bytes()) {
                info!("Successfully parsed master playlist from {}", uri);
                self.handle_playlist(&all_data, uri).await?;
            } else if let Ok((_, media_playlist)) = parse_media_playlist(playlist_str.as_bytes()) {
                info!("Successfully parsed media playlist from {}", uri);
                self.handle_playlist(&all_data, uri).await?;
            } else {
                warn!("Failed to parse playlist from {}", uri);
                debug!("Failed playlist content:\n{}", playlist_str);
            }
        }

        Ok(all_data)
    }

    // 添加一个辅助方法来生成哈希
    fn hash_string(&self, s: &str) -> String {
        let mut hasher = DefaultHasher::new();
        s.hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    }

    // 修改缓存路径生成方法
    fn make_cache_path(&self, uri: &str) -> PathBuf {
        // 生成哈希作为文件名
        let hash = self.hash_string(uri);
        
        // 使用相对于当前目录的路径
        let cache_dir = std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join("cache/hls");
            
        // 确保缓存目录存在
        if !cache_dir.exists() {
            if let Err(e) = std::fs::create_dir_all(&cache_dir) {
                warn!("Failed to create cache directory: {}", e);
            }
        }
        
        // 使用哈希作为文件名，但保留原始扩展名
        let extension = Path::new(uri)
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("");
            
        if extension.is_empty() {
            cache_dir.join(hash)
        } else {
            cache_dir.join(format!("{}.{}", hash, extension))
        }
    }

    // 修改缓存键生成方法
    fn make_cache_key(&self, uri: &str) -> String {
        format!("hls:{}", self.hash_string(uri))
    }

    // 添加一个方法来保存 URL 到哈希的映射
    async fn save_url_mapping(&self, uri: &str, hash: &str) -> Result<(), PluginError> {
        let mapping_file = std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join("cache/hls/url_mapping.json");
            
        let mut mappings = if mapping_file.exists() {
            let content = tokio::fs::read_to_string(&mapping_file).await
                .map_err(|e| PluginError::Storage(format!("Failed to read mapping file: {}", e)))?;
            serde_json::from_str(&content)
                .unwrap_or_else(|_| HashMap::new())
        } else {
            HashMap::new()
        };

        mappings.insert(hash.to_string(), uri.to_string());

        let content = serde_json::to_string_pretty(&mappings)
            .map_err(|e| PluginError::Storage(format!("Failed to serialize mappings: {}", e)))?;

        tokio::fs::write(&mapping_file, content).await
            .map_err(|e| PluginError::Storage(format!("Failed to write mapping file: {}", e)))?;

        Ok(())
    }

    async fn handle_playlist(&self, data: &[u8], base_uri: &str) -> Result<(), PluginError> {
        let playlist_str = String::from_utf8_lossy(data);
        info!("Processing playlist: {}", base_uri);
        
        // 获取基础 URL 的路径部分
        let base_url = if let Ok(parsed) = url::Url::parse(base_uri) {
            parsed
        } else {
            return Err(PluginError::Network("Invalid base URL".into()));
        };

        debug!("Playlist content:\n{}", playlist_str);
        
        if let Ok((_, master_playlist)) = parse_master_playlist(playlist_str.as_bytes()) {
            info!("Found master playlist with {} variants", master_playlist.variants.len());
            for variant in &master_playlist.variants {
                // 合并相对路径
                let full_url = base_url.join(&variant.uri)
                    .map_err(|e| PluginError::Network(format!("Failed to join URL: {}", e)))?;
                info!("Processing variant playlist: {}", full_url);
                
                // 使用正确的缓存路径
                let cache_key = self.make_cache_key(&full_url.to_string());
                let file_path = self.make_cache_path(&full_url.to_string());
                
                // 确保目录存在
                if let Some(parent) = file_path.parent() {
                    if !parent.exists() {
                        info!("Creating directory: {:?}", parent);
                        tokio::fs::create_dir_all(parent).await
                            .map_err(|e| {
                                error!("Failed to create directory {:?}: {}", parent, e);
                                PluginError::Storage(format!("Failed to create directory: {}", e))
                            })?;
                    }
                }

                // 下载并处理变体播放列表
                let variant_data = self.download_file(&full_url.to_string()).await?;
                Box::pin(self.handle_playlist(&variant_data, &full_url.to_string())).await?;
            }
        } else if let Ok((_, media_playlist)) = parse_media_playlist(playlist_str.as_bytes()) {
            info!("Found media playlist with {} segments", media_playlist.segments.len());
            for segment in &media_playlist.segments {
                // 合并相对路径
                let full_url = base_url.join(&segment.uri)
                    .map_err(|e| PluginError::Network(format!("Failed to join URL: {}", e)))?;
                info!("Downloading video segment: {}", full_url);
                
                // 使用正确的缓存路径
                let cache_key = self.make_cache_key(&full_url.to_string());
                let file_path = self.make_cache_path(&full_url.to_string());
                
                // 确保目录存在
                if let Some(parent) = file_path.parent() {
                    if !parent.exists() {
                        info!("Creating directory: {:?}", parent);
                        tokio::fs::create_dir_all(parent).await
                            .map_err(|e| {
                                error!("Failed to create directory {:?}: {}", parent, e);
                                PluginError::Storage(format!("Failed to create directory: {}", e))
                            })?;
                    }
                }

                // 下载视频片段
                let segment_data = self.download_file(&full_url.to_string()).await?;
                
                // 缓存视频片段
                let metadata = CacheMetadata::new("video/mp2t".into());
                self.cache.store(cache_key, segment_data, metadata).await?;
                
                info!("Successfully cached segment: {}", full_url);
            }
        }

        // 保存 URL 映射
        let hash = self.hash_string(base_uri);
        self.save_url_mapping(base_uri, &hash).await?;

        Ok(())
    }

    // 添加一个辅助方法来下载文件
    async fn download_file(&self, uri: &str) -> Result<Vec<u8>, PluginError> {
        let client = self.create_client(uri).await?;
        let uri_parsed = uri.parse::<hyper::Uri>()
            .map_err(|e| PluginError::Network(format!("Invalid URL: {}", e)))?;
        
        let mut response = client.get(uri_parsed).await
            .map_err(|e| PluginError::Network(format!("Failed to download file: {}", e)))?;

        if !response.status().is_success() {
            return Err(PluginError::Network(format!("Server returned status: {}", response.status())));
        }

        let mut data = Vec::new();
        while let Some(chunk) = response.body_mut().data().await {
            let chunk = chunk.map_err(|e| PluginError::Network(format!("Failed to read chunk: {}", e)))?;
            data.extend_from_slice(&chunk);
        }

        Ok(data)
    }

    // 添加一个辅助方法来创建合适的客户端
    async fn create_client(&self, _uri: &str) -> Result<hyper::Client<hyper_tls::HttpsConnector<HttpConnector>>, PluginError> {
        let https = hyper_tls::HttpsConnector::new();
        Ok(hyper::Client::builder()
            .pool_idle_timeout(Duration::from_secs(30))
            .build::<_, hyper::Body>(https))
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
        let client = self.create_client(uri).await?;

        let uri_parsed = uri.parse::<hyper::Uri>()
            .map_err(|e| PluginError::Network(format!("Invalid URL: {}", e)))?;
        
        let mut response = match timeout(Duration::from_secs(30), client.get(uri_parsed.clone())).await {
            Ok(result) => match result {
                Ok(resp) => resp,
                Err(e) => {
                    error!("Failed to download {}: {}", uri, e);
                    return Err(PluginError::Network(format!("Request failed: {}", e)));
                }
            },
            Err(_) => {
                error!("Request timeout for {}", uri);
                return Err(PluginError::Network("Request timeout".into()));
            }
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
        
        // 创建缓存流
        if let Err(e) = self.cache.store_stream(cache_key.clone(), metadata).await {
            warn!("Failed to create cache stream: {}", e);
            // 继续处理，即使缓存失败
        }

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

                            // 处理 100 个块输出一次进度
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

    async fn stream_request(&self, uri: &str, sender: hyper::body::Sender) -> Result<(), PluginError> {
        self.stream_request(uri, sender).await
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