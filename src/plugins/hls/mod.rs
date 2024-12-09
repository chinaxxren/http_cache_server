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
use chrono::{DateTime, Utc};

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
        
        // 创建结构
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
                // 解析分片时
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
                        downloading: false,
                        bytes_downloaded: 0,
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
        info!("Downloaded {} bytes from {}", body.len(), uri);
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
            if info.segments.len() > 10 { // 留最近10个片
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
                    let segments_to_prefetch: Vec<Segment> = info.segments.iter()
                        .take(3)
                        .filter(|s| !s.cached && !s.downloading)
                        .cloned()
                        .collect();

                    if !segments_to_prefetch.is_empty() {
                        self.prefetch_segments(segments_to_prefetch, 3).await;
                    }
                }
            }
        }
        Ok(())
    }

    /// 根据当前带宽选择合适的变体流
    async fn select_variant<'a>(&self, playlist_url: &str, variants: &'a [Variant]) -> Option<&'a Variant> {
        let bandwidth = self.adaptive_manager.get_bandwidth(playlist_url).await;
        let target_bandwidth = bandwidth * 0.8; // 使用80%带宽作为目标
        
        variants.iter()
            .rev() // 从高到低排序
            .find(|v| v.bandwidth as f64 <= target_bandwidth)
            .or_else(|| variants.first()) // 如果都不满足，选择最低质量
    }

    /// 更新带宽统计
    async fn update_bandwidth_stats(&self, uri: &str, bytes: u64, duration: Duration) {
        if uri.ends_with(".ts") {
            if let Ok(playlist_url) = self.get_playlist_url(uri) {
                let bps = (bytes as f64 * 8.0) / duration.as_secs_f64();
                self.adaptive_manager.update_bandwidth(&playlist_url, bps).await;
                
                // 记录带宽信息
                debug!("Bandwidth stats for {}: {:.2} Mbps", uri, bps / 1_000_000.0);
            }
        }
    }

    /// 预取分片
    async fn prefetch_segments(&self, segments: Vec<Segment>, count: usize) {
        for segment in segments.into_iter().take(count) {
            if !segment.cached && !segment.downloading {
                debug!("Scheduling segment prefetch: {}", segment.url);
                if let Err(e) = self.prefetch_tx.send(segment.url.clone()).await {
                    warn!("Failed to schedule segment prefetch: {}", e);
                }
            }
        }
    }

    /// 智能预取
    async fn smart_prefetch(&self, uri: &str) -> Result<(), PluginError> {
        let state = self.state.read().await;
        
        // 如果是主播放列表，预取最适合的变体流
        if let Some(info) = state.playlists.values()
            .find(|info| info.url == uri && matches!(info.playlist_type, PlaylistType::Master)) {
            if let Some(variant) = self.select_variant(uri, &info.variants).await {
                debug!("Selected variant for prefetch: {} ({} bps)", variant.url, variant.bandwidth);
                self.prefetch_tx.send(variant.url.clone()).await
                    .unwrap_or_else(|e| warn!("Failed to schedule variant prefetch: {}", e));
            }
        }
        
        // 如果是媒体播放列表，预取未缓存的分片
        if let Some(info) = state.playlists.values()
            .find(|info| info.url == uri && matches!(info.playlist_type, PlaylistType::Media)) {
            let uncached_segments: Vec<Segment> = info.segments.iter()
                .filter(|s| !s.cached && !s.downloading)
                .cloned()  // 克隆 Segment
                .collect();
                
            // 预取前3个未缓存的分片
            if !uncached_segments.is_empty() {
                self.prefetch_segments(uncached_segments, 3).await;
            }
        }
        
        Ok(())
    }

    /// 获取播放列表状态
    pub async fn get_playlist_status(&self, uri: &str) -> Result<PlaylistStatus, PluginError> {
        let state = self.state.read().await;
        let hash = self.hash_url(uri);
        
        if let Some(info) = state.playlists.get(&hash) {
            let total_segments = info.segments.len();
            let cached_segments = info.segments.iter().filter(|s| s.cached).count();
            let downloading_segments = info.segments.iter().filter(|s| s.downloading).count();
            
            Ok(PlaylistStatus {
                url: info.url.clone(),
                playlist_type: info.playlist_type.clone(),
                total_segments,
                cached_segments,
                downloading_segments,
                variants: info.variants.len(),
                last_modified: info.last_modified,
                bandwidth: self.adaptive_manager.get_bandwidth(&info.url).await,
            })
        } else {
            Err(PluginError::Hls(format!("Playlist not found: {}", uri)))
        }
    }

    /// 下载内容（带重试）
    async fn download_with_retry(&self, uri: &str, max_retries: u32) -> Result<Vec<u8>, PluginError> {
        let mut retries = 0;
        loop {
            match self.download_content(uri).await {
                Ok(content) => return Ok(content),
                Err(e) => {
                    retries += 1;
                    if retries >= max_retries {
                        return Err(e);
                    }
                    warn!("Download failed for {}, retry {}/{}: {}", uri, retries, max_retries, e);
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        }
    }

    /// 安全的状态更新
    async fn update_state<F, T>(&self, f: F) -> Result<T, PluginError>
    where
        F: FnOnce(&mut HLSState) -> Result<T, PluginError>,
    {
        let mut state = self.state.write().await;
        let result = f(&mut state)?;
        self.save_state().await?;
        Ok(result)
    }

    /// 清理过期的播放列表和分片
    async fn cleanup_expired(&self) -> Result<(), PluginError> {
        self.update_state(|state| {
            // 清理过期的播放列表
            let now = chrono::Utc::now();
            state.playlists.retain(|_, info| {
                let age = now - info.last_modified;
                age < chrono::Duration::hours(1)
            });

            // 清理过期的分片
            for info in state.playlists.values_mut() {
                if info.segments.len() > 10 {
                    let to_remove = info.segments.len() - 10;
                    info.segments.drain(0..to_remove);
                }
            }
            Ok(())
        }).await
    }

    /// 获取缓存统计信息
    pub async fn get_cache_stats(&self) -> Result<CacheStats, PluginError> {
        let state = self.state.read().await;
        let mut total_segments = 0;
        let mut cached_segments = 0;
        let mut downloading_segments = 0;
        let mut total_size = 0;

        for info in state.playlists.values() {
            total_segments += info.segments.len();
            cached_segments += info.segments.iter().filter(|s| s.cached).count();
            downloading_segments += info.segments.iter().filter(|s| s.downloading).count();
            total_size += info.segments.iter()
                .filter_map(|s| s.size)
                .sum::<u64>();
        }

        Ok(CacheStats {
            total_playlists: state.playlists.len(),
            total_segments,
            cached_segments,
            downloading_segments,
            total_size,
        })
    }

    /// 记录性能指标
    async fn record_metrics(&self, uri: &str, bytes: u64, duration: Duration) {
        if let Ok(playlist_url) = self.get_playlist_url(uri) {
            let bps = (bytes as f64 * 8.0) / duration.as_secs_f64();
            self.adaptive_manager.update_bandwidth(&playlist_url, bps).await;
            
            info!("Performance metrics for {}:", uri);
            info!("  - Size: {:.2} MB", bytes as f64 / (1024.0 * 1024.0));
            info!("  - Duration: {:.2}s", duration.as_secs_f64());
            info!("  - Bandwidth: {:.2} Mbps", bps / 1_000_000.0);
        }
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
        
        // 打印返回content转为字符串内容
        // let content_str = String::from_utf8_lossy(&content);
        // debug!("Returning content: {}", content_str);
        
        self.cache.store(cache_key, content.clone(), metadata).await?;

        Ok(content)
    }

    async fn stream_request(&self, uri: &str, mut sender: hyper::body::Sender) -> Result<(), PluginError> {
        let cache_key = self.make_cache_key(uri);
        
        // 如果是 m3u8 文件，直接返回完整内容
        if uri.ends_with(".m3u8") {
            match self.cache.get(&cache_key).await {
                Ok((data, _)) => {
                    debug!("Cache hit for playlist {}", uri);
                    sender.send_data(bytes::Bytes::from(data)).await
                        .map_err(|e| PluginError::Network(format!("Failed to send cached playlist: {}", e)))?;
                }
                Err(_) => {
                    debug!("Cache miss for playlist {}, downloading...", uri);
                    let content = self.handle_request(uri).await?;
                    sender.send_data(bytes::Bytes::from(content)).await
                        .map_err(|e| PluginError::Network(format!("Failed to send playlist: {}", e)))?;
                }
            }
            return Ok(());
        }

        // 处理 .ts 分片
        debug!("Streaming segment {}", uri);

        // 更新分片状态为开始下载
        if uri.ends_with(".ts") {
            let mut state = self.state.write().await;
            if let Some(info) = state.playlists.values_mut()
                .find(|info| info.segments.iter().any(|s| s.url == uri)) {
                if let Some(segment) = info.segments.iter_mut().find(|s| s.url == uri) {
                    segment.downloading = true;
                    segment.bytes_downloaded = 0;
                }
            }
            self.save_state().await?;
        }

        let metadata = CacheMetadata::new("video/mp2t".into());
        self.cache.store_stream(cache_key.clone(), metadata).await?;

        // 开始下载
        let client = hyper::Client::new();
        let resp = client.get(uri.parse().unwrap())
            .await
            .map_err(|e| PluginError::Network(e.to_string()))?;

        let status = resp.status();
        let content_length = resp.headers()
            .get(hyper::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());

        if !status.is_success() {
            return Err(PluginError::Network(format!("Server returned status: {}", status)));
        }

        let mut body = resp.into_body();
        let mut total_bytes = 0;
        let start_time = std::time::Instant::now();
        let mut last_update = start_time;

        // 流式传输
        while let Some(chunk) = hyper::body::HttpBody::data(&mut body).await {
            let chunk = chunk.map_err(|e| PluginError::Network(e.to_string()))?;
            let chunk_size = chunk.len();
            total_bytes += chunk_size;

            // 新分片下载进度
            if uri.ends_with(".ts") {
                let now = std::time::Instant::now();
                if now.duration_since(last_update) >= Duration::from_secs(1) {
                    let mut state = self.state.write().await;
                    if let Some(info) = state.playlists.values_mut()
                        .find(|info| info.segments.iter().any(|s| s.url == uri)) {
                        if let Some(segment) = info.segments.iter_mut().find(|s| s.url == uri) {
                            segment.bytes_downloaded = total_bytes as u64;
                        }
                    }
                    self.save_state().await?;
                    last_update = now;
                }
            }

            // 缓存分片
            self.cache.append_chunk(&cache_key, &chunk).await?;

            // 发送给客户端
            sender.send_data(chunk.into()).await
                .map_err(|e| PluginError::Network(format!("Failed to send chunk: {}", e)))?;

            // 每 1MB 更新一次进度
            if total_bytes % (1024 * 1024) == 0 {
                let elapsed = start_time.elapsed();
                let speed = total_bytes as f64 / (1024.0 * 1024.0 * elapsed.as_secs_f64());
                let progress = content_length.map_or(0.0, |len| (total_bytes as f64 / len as f64) * 100.0);
                debug!("Streaming progress: {:.1}%, {:.2} MB @ {:.2} MB/s", 
                    progress,
                    total_bytes as f64 / (1024.0 * 1024.0),
                    speed
                );
            }
        }

        // 完成缓存和更新状态
        self.cache.finish_stream(&cache_key).await?;

        // 更新分片状态为完成
        if uri.ends_with(".ts") {
            let mut state = self.state.write().await;
            if let Some(info) = state.playlists.values_mut()
                .find(|info| info.segments.iter().any(|s| s.url == uri)) {
                if let Some(segment) = info.segments.iter_mut().find(|s| s.url == uri) {
                    segment.downloading = false;
                    segment.cached = true;
                    segment.size = Some(total_bytes as u64);
                }
            }
            self.save_state().await?;
        }

        let elapsed = start_time.elapsed();
        let speed = total_bytes as f64 / (1024.0 * 1024.0 * elapsed.as_secs_f64());
        debug!("Streaming completed: {:.2} MB in {:.2}s @ {:.2} MB/s", 
            total_bytes as f64 / (1024.0 * 1024.0),
            elapsed.as_secs_f64(),
            speed
        );

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

    /// 生成缓存键
    fn make_cache_key(&self, uri: &str) -> String {
        let hash = self.hash_url(uri);
        format!("hls:{}", hash)
    }

    fn get_content_type(&self, uri: &str) -> String {
        if uri.ends_with(".m3u8") {
            "application/vnd.apple.mpegurl".to_string()
        } else {
            "video/mp2t".to_string()
        }
    }
} 

#[derive(Debug)]
pub struct PlaylistStatus {
    pub url: String,
    pub playlist_type: PlaylistType,
    pub total_segments: usize,
    pub cached_segments: usize,
    pub downloading_segments: usize,
    pub variants: usize,
    pub last_modified: DateTime<Utc>,
    pub bandwidth: f64,
} 

#[derive(Debug)]
pub struct CacheStats {
    pub total_playlists: usize,
    pub total_segments: usize,
    pub cached_segments: usize,
    pub downloading_segments: usize,
    pub total_size: u64,
} 