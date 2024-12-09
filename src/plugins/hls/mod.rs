mod types;

use std::sync::Arc;
use std::path::PathBuf;
use tokio::sync::{RwLock, mpsc};
use tracing::{debug, info, warn, error};
use async_trait::async_trait;
use hyper::body::HttpBody;
use tokio::time::{timeout, Duration};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use crate::error::PluginError;
use crate::plugin::Plugin;
use crate::plugins::cache::{CacheManager, CacheMetadata};
use crate::proxy::MediaHandler;

use self::types::{
    HLSState, PlaylistInfo, PlaylistType, PathBuilder,
    Variant, Segment,
};

#[derive(Debug)]
pub struct HLSPlugin {
    state: Arc<RwLock<HLSState>>,
    cache: Arc<CacheManager>,
    path_builder: PathBuilder,
    prefetch_tx: mpsc::Sender<String>,
} 

impl HLSPlugin {
    pub fn new(cache: Arc<CacheManager>) -> Self {
        let (tx, mut rx) = mpsc::channel(100);
        let path_builder = PathBuilder::new("cache/hls");
        let state = Arc::new(RwLock::new(HLSState::default()));
        
        // 创建目录结构
        std::fs::create_dir_all("cache/hls").unwrap_or_else(|e| {
            error!("Failed to create HLS cache directory: {}", e);
        });

        // 加载状态
        if let Ok(content) = std::fs::read_to_string("cache/hls/state.json") {
            if let Ok(loaded_state) = serde_json::from_str(&content) {
                *state.blocking_write() = loaded_state;
            }
        }

        // 克隆发送端用于预取任务
        let tx_clone = tx.clone();

        // 启动预取任务
        let state_clone = state.clone();
        let cache_clone = cache.clone();
        let path_builder_clone = path_builder.clone();
        tokio::spawn(async move {
            while let Some(url) = rx.recv().await {
                debug!("Received prefetch request for {}", url);
                let plugin = HLSPlugin {
                    state: state_clone.clone(),
                    cache: cache_clone.clone(),
                    path_builder: path_builder_clone.clone(),
                    prefetch_tx: tx_clone.clone(),
                };
                if let Err(e) = plugin.handle_request(&url).await {
                    warn!("Failed to prefetch {}: {}", url, e);
                }
            }
        });

        Self {
            state,
            cache,
            path_builder,
            prefetch_tx: tx,
        }
    }

    /// 生成 URL 哈希
    fn hash_url(&self, url: &str) -> String {
        let mut hasher = DefaultHasher::new();
        url.hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    }

    /// 保存状态
    async fn save_state(&self) -> Result<(), PluginError> {
        let state = self.state.read().await;
        let content = serde_json::to_string_pretty(&*state)
            .map_err(|e| PluginError::Storage(format!("Failed to serialize state: {}", e)))?;
        
        let temp_path = PathBuf::from("cache/hls/state.json.tmp");
        let final_path = PathBuf::from("cache/hls/state.json");

        tokio::fs::write(&temp_path, content).await
            .map_err(|e| PluginError::Storage(format!("Failed to write state file: {}", e)))?;

        tokio::fs::rename(temp_path, final_path).await
            .map_err(|e| PluginError::Storage(format!("Failed to save state file: {}", e)))?;

        Ok(())
    }

    /// 解析 m3u8 内容
    async fn parse_playlist(&self, content: &[u8], url: &str) -> Result<(PlaylistType, PlaylistInfo), PluginError> {
        let content_str = String::from_utf8_lossy(content);
        let hash = self.hash_url(url);
        let mut info = PlaylistInfo {
            url: url.to_string(),
            hash: hash.clone(),
            playlist_type: PlaylistType::Media, // 默认为媒体播放列表
            last_modified: chrono::Utc::now(),
            target_duration: None,
            media_sequence: None,
            variants: Vec::new(),
            segments: Vec::new(),
        };

        let mut is_master = false;
        let mut current_bandwidth = None;
        let mut current_resolution = None;

        for line in content_str.lines() {
            if line.starts_with("#EXT-X-STREAM-INF:") {
                is_master = true;
                // 解析带宽和分辨率
                for attr in line[18..].split(',') {
                    if attr.starts_with("BANDWIDTH=") {
                        current_bandwidth = attr[10..].parse().ok();
                    } else if attr.starts_with("RESOLUTION=") {
                        current_resolution = Some(attr[11..].to_string());
                    }
                }
            } else if line.starts_with("#EXT-X-TARGETDURATION:") {
                if let Ok(duration) = line[22..].parse() {
                    info.target_duration = Some(duration);
                }
            } else if line.starts_with("#EXT-X-MEDIA-SEQUENCE:") {
                if let Ok(sequence) = line[22..].parse() {
                    info.media_sequence = Some(sequence);
                }
            } else if line.starts_with("#EXTINF:") {
                // 解析分片时长
                if let Ok(duration) = line[8..].split(',').next().unwrap().parse() {
                    info.segments.push(Segment {
                        sequence: info.segments.len() as u32,
                        duration,
                        url: String::new(), // 下一行会填充
                        size: None,
                        cached: false,
                    });
                }
            } else if !line.starts_with('#') && !line.is_empty() {
                if is_master {
                    // 添加变体流
                    info.variants.push(Variant {
                        bandwidth: current_bandwidth.unwrap_or(0),
                        resolution: current_resolution.take(),
                        url: line.trim().to_string(),
                        hash: self.hash_url(line.trim()),
                    });
                } else if let Some(segment) = info.segments.last_mut() {
                    // 填充分片URL
                    segment.url = line.trim().to_string();
                }
            }
        }

        info.playlist_type = if is_master {
            PlaylistType::Master
        } else {
            PlaylistType::Media
        };

        Ok((info.playlist_type.clone(), info))
    }

    /// 下载内容
    async fn download_content(&self, uri: &str) -> Result<Vec<u8>, PluginError> {
        let client = hyper::Client::new();
        let resp = client.get(uri.parse().unwrap())
            .await
            .map_err(|e| PluginError::Network(e.to_string()))?;
            
        let body = hyper::body::to_bytes(resp.into_body())
            .await
            .map_err(|e| PluginError::Network(e.to_string()))?;
            
        Ok(body.to_vec())
    }

    /// 获取缓存路径
    fn get_cache_path(&self, uri: &str, is_master: bool) -> PathBuf {
        let hash = self.hash_url(uri);
        if is_master {
            self.path_builder.master_playlist(&hash)
        } else {
            self.path_builder.media_playlist(&hash)
        }
    }

    /// 获取分片缓存路径
    fn get_segment_path(&self, playlist_hash: &str, variant: Option<&str>, sequence: u32) -> PathBuf {
        self.path_builder.segment(playlist_hash, variant, sequence)
    }
} 

impl Clone for HLSPlugin {
    fn clone(&self) -> Self {
        Self {
            state: self.state.clone(),
            cache: self.cache.clone(),
            path_builder: self.path_builder.clone(),
            prefetch_tx: self.prefetch_tx.clone(),
        }
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
        info!("Initializing HLS plugin");
        Ok(())
    }

    async fn cleanup(&self) -> Result<(), PluginError> {
        info!("Cleaning up HLS plugin");
        self.save_state().await
    }
} 

#[async_trait]
impl MediaHandler for HLSPlugin {
    async fn handle_request(&self, uri: &str) -> Result<Vec<u8>, PluginError> {
        debug!("Handling HLS request: {}", uri);
        let hash = self.hash_url(uri);
        
        // 检查缓存
        let cache_key = format!("hls:{}", uri);
        if let Ok((data, _)) = self.cache.get(&cache_key).await {
            debug!("Cache hit for {}", uri);
            return Ok(data);
        }

        // 下载内容
        debug!("Cache miss for {}, downloading...", uri);
        let content = self.download_content(uri).await?;

        // 处理播放列表
        if uri.ends_with(".m3u8") {
            let (playlist_type, info) = self.parse_playlist(&content, uri).await?;
            
            // 更新状态
            let mut state = self.state.write().await;
            state.playlists.insert(hash.clone(), info);
            self.save_state().await?;

            // 预取变体流或分片
            match playlist_type {
                PlaylistType::Master => {
                    // 预取所有变体流
                    if let Some(info) = state.playlists.get(&hash) {
                        for variant in &info.variants {
                            self.prefetch_tx.send(variant.url.clone()).await
                                .unwrap_or_else(|e| warn!("Failed to schedule variant prefetch: {}", e));
                        }
                    }
                }
                PlaylistType::Media => {
                    // 预取前几个分片
                    if let Some(info) = state.playlists.get(&hash) {
                        for segment in info.segments.iter().take(3) {
                            self.prefetch_tx.send(segment.url.clone()).await
                                .unwrap_or_else(|e| warn!("Failed to schedule segment prefetch: {}", e));
                        }
                    }
                }
            }
        }

        // 缓存内容
        let metadata = CacheMetadata::new(
            if uri.ends_with(".m3u8") {
                "application/vnd.apple.mpegurl"
            } else {
                "video/mp2t"
            }.to_string()
        );
        self.cache.store(cache_key, content.clone(), metadata).await?;

        Ok(content)
    }

    fn can_handle(&self, uri: &str) -> bool {
        uri.ends_with(".m3u8") || uri.ends_with(".ts")
    }

    async fn stream_request(&self, uri: &str, mut sender: hyper::body::Sender) -> Result<(), PluginError> {
        let content = self.handle_request(uri).await?;
        
        // 分块发送
        const CHUNK_SIZE: usize = 64 * 1024;  // 64KB chunks
        let mut offset = 0;
        
        while offset < content.len() {
            let end = (offset + CHUNK_SIZE).min(content.len());
            let chunk = content[offset..end].to_vec();
            
            sender.send_data(bytes::Bytes::from(chunk)).await
                .map_err(|e| PluginError::Network(format!("Failed to send chunk: {}", e)))?;
                
            offset = end;
        }

        Ok(())
    }
} 