mod types;
mod adaptive;

use std::sync::Arc;
use std::path::PathBuf;
use tokio::sync::{RwLock, mpsc};
use tracing::{debug, info, warn, error};
use async_trait::async_trait;
use tokio::time::Duration;
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
use self::adaptive::AdaptiveStreamManager;

#[derive(Debug)]
pub struct HLSPlugin {
    state: Arc<RwLock<HLSState>>,
    cache: Arc<CacheManager>,
    path_builder: PathBuilder,
    prefetch_tx: mpsc::Sender<String>,
    adaptive_manager: Arc<AdaptiveStreamManager>,
} 

impl HLSPlugin {
    pub fn new(cache: Arc<CacheManager>) -> Self {
        let (tx, mut rx) = mpsc::channel(100);
        let path_builder = PathBuilder::new("cache/hls");
        let state = Arc::new(RwLock::new(HLSState::default()));
        let adaptive_manager = Arc::new(AdaptiveStreamManager::new());
        
        // 创建���录结构
        tokio::task::block_in_place(|| {
            if let Err(e) = std::fs::create_dir_all("cache/hls") {
                error!("Failed to create HLS cache directory: {}", e);
            }
        });

        // 加载状态
        tokio::task::block_in_place(|| {
            if let Ok(content) = std::fs::read_to_string("cache/hls/state.json") {
                if let Ok(loaded_state) = serde_json::from_str(&content) {
                    let mut state_guard = state.blocking_write();
                    *state_guard = loaded_state;
                }
            }
        });

        // 克隆用于任务的值
        let state_clone = state.clone();
        let cache_clone = cache.clone();
        let path_builder_clone = path_builder.clone();
        let adaptive_manager_clone = adaptive_manager.clone();
        let tx_clone = tx.clone();

        // 启动预取任务
        tokio::spawn(async move {
            while let Some(url) = rx.recv().await {
                debug!("Received prefetch request for {}", url);
                let plugin = HLSPlugin {
                    state: state_clone.clone(),
                    cache: cache_clone.clone(),
                    path_builder: path_builder_clone.clone(),
                    prefetch_tx: tx_clone.clone(),
                    adaptive_manager: adaptive_manager_clone.clone(),
                };
                if let Err(e) = plugin.handle_request(&url).await {
                    warn!("Failed to prefetch {}: {}", url, e);
                }
            }
        });

        // 克隆用于清理任务
        let state_clone = state.clone();

        // 启动清理任务
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(300)).await;
                let mut state = state_clone.write().await;
                
                // 清理过期的播放列表
                state.playlists.retain(|_, info| {
                    let age = chrono::Utc::now() - info.last_modified;
                    age < chrono::Duration::hours(1)
                });

                // 清理旧的分片
                for info in state.playlists.values_mut() {
                    if info.segments.len() > 10 {
                        let to_remove = info.segments.len() - 10;
                        info.segments.drain(0..to_remove);
                    }
                }
            }
        });

        Self {
            state,
            cache,
            path_builder,
            prefetch_tx: tx,
            adaptive_manager,
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
            playlist_type: PlaylistType::Media,
            last_modified: chrono::Utc::now(),
            target_duration: None,
            media_sequence: None,
            variants: Vec::new(),
            segments: Vec::new(),
        };

        let mut is_master = false;
        let mut current_bandwidth = None;
        let mut current_resolution = None;
        let mut current_duration = None;

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
                    current_duration = Some(duration);
                }
            } else if !line.starts_with('#') && !line.is_empty() {
                let line = line.trim();
                if is_master {
                    // 添加变体流
                    let variant_url = self.resolve_url(url, line);
                    info.variants.push(Variant {
                        bandwidth: current_bandwidth.unwrap_or(0),
                        resolution: current_resolution.take(),
                        url: variant_url.clone(),
                        hash: self.hash_url(&variant_url),
                    });
                } else if let Some(duration) = current_duration.take() {
                    // 添加分片，使用绝对URL
                    let segment_url = self.resolve_url(url, line);
                    info.segments.push(Segment {
                        sequence: info.segments.len() as u32,
                        duration,
                        url: segment_url,
                        size: None,
                        cached: false,
                    });
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

    /// 将相对URL转换为绝对URL
    fn resolve_url(&self, base_url: &str, relative_url: &str) -> String {
        if relative_url.starts_with("http://") || relative_url.starts_with("https://") {
            relative_url.to_string()
        } else {
            let base = url::Url::parse(base_url).unwrap();
            base.join(relative_url).unwrap().to_string()
        }
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

    async fn prefetch_variants(&self, playlist_url: &str, variants: &[Variant]) -> Result<(), PluginError> {
        // 注册新
        self.adaptive_manager.register_stream(
            playlist_url.to_string(),
            1_000_000.0 // 初始带宽 1Mbps
        ).await;

        // 根据当前带宽选择体
        let bandwidth = self.adaptive_manager.get_bandwidth(playlist_url).await;
        let target_bandwidth = bandwidth * 0.8; // 使用80%带宽作为目标

        // 选择合适的变体
        if let Some(variant) = variants.iter()
            .rev()
            .find(|v| v.bandwidth as f64 <= target_bandwidth) {
            self.prefetch_tx.send(variant.url.clone()).await
                .unwrap_or_else(|e| warn!("Failed to schedule variant prefetch: {}", e));
        }

        Ok(())
    }

    async fn cleanup_old_segments(&self) -> Result<(), PluginError> {
        let mut state = self.state.write().await;
        for info in state.playlists.values_mut() {
            if info.segments.len() > 10 { // 保留最近10个分片
                let to_remove = info.segments.len() - 10;
                info.segments.drain(0..to_remove);
            }
        }
        self.save_state().await
    }

    fn check_cache_structure(&self) -> Result<(), PluginError> {
        let base = self.path_builder.base_path();
        let dirs = [
            "playlists/master",
            "playlists/variants",
            "segments",
        ];

        for dir in dirs {
            let path = base.join(dir);
            if !path.exists() {
                std::fs::create_dir_all(&path)
                    .map_err(|e| PluginError::Storage(format!("Failed to create directory {}: {}", path.display(), e)))?;
            }
        }

        Ok(())
    }

    async fn monitor_bandwidth(&self, uri: &str, size: usize, duration: Duration) -> Result<(), PluginError> {
        if uri.ends_with(".ts") {
            let playlist_url = self.get_playlist_url(uri)?;
            let bps = (size as f64 * 8.0) / duration.as_secs_f64();
            self.adaptive_manager.update_bandwidth(&playlist_url, bps).await;
        }
        Ok(())
    }

    /// 处理播放列表内容
    async fn handle_playlist(&self, uri: &str, content: &[u8]) -> Result<(), PluginError> {
        if let Ok((playlist_type, info)) = self.parse_playlist(content, uri).await {
            // 更新状态
            let mut state = self.state.write().await;
            state.playlists.insert(info.hash.clone(), info.clone());
            drop(state);
            self.save_state().await?;

            // 预取处理
            match playlist_type {
                PlaylistType::Master => {
                    for variant in &info.variants {
                        debug!("Scheduling variant prefetch: {}", variant.url);
                        if let Err(e) = self.prefetch_tx.send(variant.url.clone()).await {
                            warn!("Failed to schedule variant prefetch: {}", e);
                        }
                    }
                }
                PlaylistType::Media => {
                    for segment in info.segments.iter().take(3) {
                        debug!("Scheduling segment prefetch: {}", segment.url);
                        if let Err(e) = self.prefetch_tx.send(segment.url.clone()).await {
                            warn!("Failed to schedule segment prefetch: {}", e);
                        }
                    }
                }
            }
        }
        Ok(())
    }
} 

impl Clone for HLSPlugin {
    fn clone(&self) -> Self {
        Self {
            state: self.state.clone(),
            cache: self.cache.clone(),
            path_builder: self.path_builder.clone(),
            prefetch_tx: self.prefetch_tx.clone(),
            adaptive_manager: self.adaptive_manager.clone(),
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
        self.check_cache_structure()?;
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
        
        let cache_key = self.make_cache_key(uri);
        
        // 检查缓存
        if let Ok((data, _)) = self.cache.get(&cache_key).await {
            debug!("Cache hit for {}", uri);
            return Ok(data);
        }

        // 下载内容
        debug!("Cache miss for {}, downloading...", uri);
        let content = self.download_content(uri).await?;

        // 处理播放列表
        if uri.ends_with(".m3u8") {
            self.handle_playlist(uri, &content).await?;
        }

        // 缓存内容
        let metadata = CacheMetadata::new(self.get_content_type(uri));
        debug!("Storing content in cache: {}", cache_key);
        self.cache.store(cache_key, content.clone(), metadata).await?;

        Ok(content)
    }

    async fn stream_request(&self, uri: &str, mut sender: hyper::body::Sender) -> Result<(), PluginError> {
        let cache_key = format!("hls:{}", uri);
        
        match self.cache.get(&cache_key).await {
            Ok((data, _)) => {
                debug!("Cache hit for streaming {}", uri);
                sender.send_data(bytes::Bytes::from(data)).await
                    .map_err(|e| PluginError::Network(format!("Failed to send cached data: {}", e)))?;
            }
            Err(_) => {
                debug!("Cache miss for streaming {}, downloading...", uri);
                let content = self.handle_request(uri).await?;
                sender.send_data(bytes::Bytes::from(content)).await
                    .map_err(|e| PluginError::Network(format!("Failed to send data: {}", e)))?;
            }
        }

        Ok(())
    }

    fn can_handle(&self, uri: &str) -> bool {
        uri.ends_with(".m3u8") || uri.ends_with(".ts")
    }
} 

impl HLSPlugin {
    /// 获取播放列表URL
    fn get_playlist_url(&self, segment_url: &str) -> Result<String, PluginError> {
        let state = self.state.blocking_read();
        for info in state.playlists.values() {
            if info.segments.iter().any(|s| s.url == segment_url) {
                return Ok(info.url.clone());
            }
        }
        Err(PluginError::Hls(format!("Cannot find playlist for segment: {}", segment_url)))
    }

    /// 从URL中提取序列号
    fn extract_sequence_number(&self, uri: &str) -> Result<u32, PluginError> {
        uri.split('/')
            .last()
            .and_then(|s| s.split('.').next())
            .and_then(|s| s.replace("segment", "").parse().ok())
            .ok_or_else(|| PluginError::Hls(format!("Invalid segment URL: {}", uri)))
    }

    /// 获取变体名称
    fn get_variant_name(&self, uri: &str) -> Option<String> {
        uri.split('/')
            .rev()
            .nth(1)
            .map(|s| s.to_string())
    }

    /// 检查是否为主播放列表
    fn is_master_playlist(&self, uri: &str) -> bool {
        let state = self.state.blocking_read();
        let hash = self.hash_url(uri);
        state.playlists.get(&hash)
            .map(|info| matches!(info.playlist_type, PlaylistType::Master))
            .unwrap_or(false)
    }

    /// 检查分片是否已缓存
    async fn is_segment_cached(&self, uri: &str) -> bool {
        let cache_key = format!("hls:{}", uri);
        self.cache.contains(&cache_key).await
    }

    /// 生成缓存键
    fn make_cache_key(&self, uri: &str) -> String {
        format!("hls:{}", uri)
    }

    fn get_content_type(&self, uri: &str) -> String {
        if uri.ends_with(".m3u8") {
            "application/vnd.apple.mpegurl".to_string()
        } else {
            "video/mp2t".to_string()
        }
    }
} 