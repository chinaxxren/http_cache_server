use std::sync::Arc;
use hyper::body::Sender;
use tokio::sync::RwLock;
use std::collections::HashMap;
use tracing::{debug, info, warn};
use crate::error::PluginError;
use crate::plugins::cache::{CacheManager, CacheMetadata};
use super::types::{PlaylistInfo, HLSState};
use std::marker::PhantomData;
use crate::utils::get_storage_paths;

#[derive(Debug)]
pub struct PlaylistManager {
    cache: Arc<CacheManager>,
    state: Arc<RwLock<HLSState>>,
    segment_manager: Arc<super::segment::SegmentManager>,
    _marker: PhantomData<()>,
}

impl PlaylistManager {
    pub fn new(cache: Arc<CacheManager>, segment_manager: Arc<super::segment::SegmentManager>) -> Self {
        Self {
            cache,
            state: Arc::new(RwLock::new(HLSState::default())),
            segment_manager,
            _marker: PhantomData,
        }
    }

    /// 保存状态到文件
    async fn save_state(&self) -> Result<(), PluginError> {
        let state = self.state.read().await;
        let content = serde_json::to_string_pretty(&*state)
            .map_err(|e| PluginError::Storage(format!("Failed to serialize state: {}", e)))?;
        
        let temp_path = std::path::PathBuf::from("cache/hls/state.json.tmp");
        let final_path = std::path::PathBuf::from("cache/hls/state.json");

        tokio::fs::write(&temp_path, content).await
            .map_err(|e| PluginError::Storage(format!("Failed to write state file: {}", e)))?;

        tokio::fs::rename(temp_path, final_path).await
            .map_err(|e| PluginError::Storage(format!("Failed to save state file: {}", e)))?;

        Ok(())
    }

    /// 更新播放列表状态
    async fn update_playlist_state(&self, info: PlaylistInfo) -> Result<(), PluginError> {
        let mut state = self.state.write().await;
        state.playlists.insert(info.hash.clone(), info);
        drop(state);
        self.save_state().await?;
        Ok(())
    }

    /// 获取播放列表信息
    pub async fn get_playlist_info(&self, uri: &str) -> Option<PlaylistInfo> {
        let state = self.state.read().await;
        let hash = super::utils::hash_url(uri);
        state.playlists.get(&hash).cloned()
    }

    /// 清理过期的播放列表和分片
    pub async fn cleanup(&self) -> Result<(), PluginError> {
        let mut state = self.state.write().await;
        let now = chrono::Utc::now();
        
        // 清理过期的播放列表
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

        drop(state);
        self.save_state().await?;
        Ok(())
    }

    /// 带重试的下载
    async fn download_with_retry(&self, uri: &str, max_retries: u32) -> Result<hyper::Response<hyper::Body>, PluginError> {
        let client = hyper::Client::new();
        let uri_parsed = uri.parse::<hyper::Uri>()
            .map_err(|e| PluginError::Network(format!("Invalid URL: {}", e)))?;
        
        let mut retries = 0;
        loop {
            match client.get(uri_parsed.clone()).await {
                Ok(resp) => return Ok(resp),
                Err(e) => {
                    retries += 1;
                    if retries >= max_retries {
                        return Err(PluginError::Network(e.to_string()));
                    }
                    warn!("Download failed for {}, retry {}/{}: {}", uri, retries, max_retries, e);
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                }
            }
        }
    }

    pub async fn handle_playlist(&self, uri: &str) -> Result<Vec<u8>, PluginError> {
        let (_, file_path) = get_storage_paths(uri);
        
        // 尝试从文件系统读取
        if let Ok(content) = tokio::fs::read(&file_path).await {
            debug!("File system hit for playlist {}", uri);
            return Ok(content);
        }

        // 下载并存储
        debug!("File system miss for playlist {}, downloading...", uri);
        self.download_playlist(uri).await
    }

    /// 内部处理播放列表的方法
    async fn handle_playlist_internal(&self, uri: &str) -> Result<Vec<u8>, PluginError> {
        let hash = super::utils::hash_url(uri);
        let base_path = std::path::PathBuf::from("cache/hls").join(&hash);
        
        // 先尝试读取处理后的播放列表
        if let Ok(content) = tokio::fs::read(base_path.join("index.m3u8")).await {
            debug!("File system hit for playlist {}", uri);
            return Ok(content);
        }

        // 再尝试读取原始播放列表
        if let Ok(content) = tokio::fs::read(base_path.join("playlist.m3u8")).await {
            debug!("File system hit for playlist {}", uri);
            return Ok(content);
        }

        // 下载并存储
        debug!("File system miss for playlist {}, downloading...", uri);
        self.download_playlist(uri).await
    }

    pub async fn stream_playlist(&self, uri: &str, mut sender: Sender) -> Result<(), PluginError> {
        let start_time = std::time::Instant::now();
        let data = self.handle_playlist(uri).await?;
        
        sender.send_data(bytes::Bytes::from(data.clone())).await
            .map_err(|e| PluginError::Network(e.to_string()))?;

        let elapsed = start_time.elapsed();
        info!(
            "Playlist {} streamed: {:.2} KB in {:.2}s",
            uri,
            data.len() as f64 / 1024.0,
            elapsed.as_secs_f64()
        );
        
        Ok(())
    }

    /// 获取缓存时间
    fn get_cache_duration(&self, info: &PlaylistInfo) -> std::time::Duration {
        match info.playlist_type {
            super::types::PlaylistType::Master => std::time::Duration::from_secs(3600), // 1小时
            super::types::PlaylistType::Media => {
                if let Some(target_duration) = info.target_duration {
                    std::time::Duration::from_secs((target_duration * 3.0) as u64) // 3倍目标时长
                } else {
                    std::time::Duration::from_secs(30) // 默认30秒
                }
            }
        }
    }

    /// 获取预取数量
    fn get_prefetch_count(&self, info: &PlaylistInfo) -> usize {
        if let Some(target_duration) = info.target_duration {
            // 预取未来10秒的分片
            (10.0 / target_duration).ceil() as usize
        } else {
            3 // 默认预取3个分片
        }
    }

    async fn download_playlist(&self, uri: &str) -> Result<Vec<u8>, PluginError> {
        let start_time = std::time::Instant::now();
        let resp = self.download_with_retry(uri, 3).await?;

        let status = resp.status();
        if !status.is_success() {
            return Err(PluginError::Network(format!("Server returned status: {}", status)));
        }

        let body = hyper::body::to_bytes(resp.into_body())
            .await
            .map_err(|e| PluginError::Network(e.to_string()))?;

        let content = body.to_vec();
        
        // 解析播放列表
        let playlist_info = self.parse_playlist(&content, uri).await?;
        
        // 更新状态
        self.update_playlist_state(playlist_info.clone()).await?;

        // 获取存储路径
        let (base_path, file_path) = get_storage_paths(uri);
        
        // 创建目录结构
        if let Some(parent) = file_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        // 保存播放列表
        tokio::fs::write(&file_path, &content).await?;

        // 如果是主播放列表，预取变体流
        if matches!(playlist_info.playlist_type, super::types::PlaylistType::Master) {
            for variant in &playlist_info.variants {
                debug!("Prefetching variant playlist: {} (bandwidth: {} bps)", 
                    variant.url, variant.bandwidth);
                // 下载变体流播放列表
                match Box::pin(self.handle_playlist_internal(&variant.url)).await {
                    Ok(variant_content) => {
                        // 解析变体流播放列表
                        if let Ok(variant_info) = self.parse_playlist(&variant_content, &variant.url).await {
                            // 预取变体流中的分片
                            if !variant_info.segments.is_empty() {
                                let prefetch_count = self.get_prefetch_count(&variant_info);
                                debug!("Prefetching first {} segments from variant {}", prefetch_count, variant.url);
                                for segment in variant_info.segments.iter().take(prefetch_count) {
                                    debug!("Prefetching segment: {} (duration: {:.2}s)", 
                                        segment.url, segment.duration);
                                    if let Err(e) = self.segment_manager.handle_segment(&segment.url).await {
                                        warn!("Failed to prefetch segment {}: {}", segment.url, e);
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => warn!("Failed to prefetch variant {}: {}", variant.url, e),
                }
            }
        }

        Ok(content)
    }

    /// 解析播放列表内容
    async fn parse_playlist(&self, content: &[u8], uri: &str) -> Result<super::types::PlaylistInfo, PluginError> {
        let content_str = String::from_utf8_lossy(content);
        
        // 检查 M3U8 头
        if !content_str.starts_with("#EXTM3U") {
            return Err(PluginError::Hls("Invalid M3U8 format: missing #EXTM3U tag".into()));
        }

        // 检查版本
        let version = content_str.lines()
            .find(|line| line.starts_with("#EXT-X-VERSION:"))
            .and_then(|line| line[15..].parse::<u32>().ok())
            .unwrap_or(1);

        debug!("Parsing HLS playlist version {}", version);

        let mut info = super::types::PlaylistInfo {
            url: uri.to_string(),
            hash: super::utils::hash_url(uri),
            playlist_type: super::types::PlaylistType::Media,
            last_modified: chrono::Utc::now(),
            target_duration: None,
            media_sequence: None,
            variants: Vec::new(),
            segments: Vec::new(),
        };

        let mut current_duration: Option<f32> = None;
        let mut current_bandwidth: Option<u64> = None;
        let mut current_resolution: Option<String> = None;
        let mut is_master = false;

        for line in content_str.lines() {
            match line {
                // 主播放列表标签
                l if l.starts_with("#EXT-X-STREAM-INF:") => {
                    is_master = true;
                    // 解析带宽和分辨率
                    for attr in l[18..].split(',') {
                        let attr = attr.trim();
                        if attr.starts_with("BANDWIDTH=") {
                            current_bandwidth = attr[10..].parse().ok();
                            debug!("Found bandwidth: {} bps", current_bandwidth.unwrap_or(0));
                        } else if attr.starts_with("RESOLUTION=") {
                            current_resolution = Some(attr[11..].to_string());
                            debug!("Found resolution: {}", attr[11..].to_string());
                        }
                    }
                },

                // 目标时长
                l if l.starts_with("#EXT-X-TARGETDURATION:") => {
                    info.target_duration = l[22..].parse().ok();
                },

                // 媒体序列号
                l if l.starts_with("#EXT-X-MEDIA-SEQUENCE:") => {
                    info.media_sequence = l[22..].parse().ok();
                },

                // 分片时长
                l if l.starts_with("#EXTINF:") => {
                    if let Some(duration_str) = l[8..].split(',').next() {
                        current_duration = duration_str.parse().ok();
                    }
                },

                // URL行（非注释行）
                l if !l.starts_with('#') && !l.is_empty() => {
                    let url = super::utils::resolve_url(uri, l.trim());
                    
                    if is_master {
                        // 添加变体
                        info.variants.push(super::types::Variant {
                            bandwidth: current_bandwidth.unwrap_or(0),
                            resolution: current_resolution.take(),
                            url: url.clone(),
                            hash: super::utils::hash_url(&url),
                        });
                    } else if let Some(duration) = current_duration.take() {
                        // 添加分片
                        info.segments.push(super::types::Segment {
                            sequence: info.segments.len() as u32,
                            duration,
                            url: url.clone(),
                            size: None,
                            cached: false,
                            downloading: false,
                            bytes_downloaded: 0,
                        });
                    }
                },

                _ => continue,
            }
        }

        info.playlist_type = if is_master {
            super::types::PlaylistType::Master
        } else {
            super::types::PlaylistType::Media
        };

        debug!("Parsed playlist {}: {} variants, {} segments",
            uri,
            info.variants.len(),
            info.segments.len()
        );

        Ok(info)
    }

    /// 获取播放列表统计信息
    pub async fn get_stats(&self) -> PlaylistStats {
        let state = self.state.read().await;
        let mut total_segments = 0;
        let mut cached_segments = 0;
        let mut downloading_segments = 0;

        for info in state.playlists.values() {
            total_segments += info.segments.len();
            cached_segments += info.segments.iter().filter(|s| s.cached).count();
            downloading_segments += info.segments.iter().filter(|s| s.downloading).count();
        }

        PlaylistStats {
            total_playlists: state.playlists.len(),
            total_segments,
            cached_segments,
            downloading_segments,
        }
    }

    /// 获取播放列表 URL
    pub async fn get_playlist_url(&self, segment_url: &str) -> Result<String, PluginError> {
        let state = self.state.read().await;
        for info in state.playlists.values() {
            if info.segments.iter().any(|s| s.url == segment_url) {
                return Ok(info.url.clone());
            }
        }
        Err(PluginError::Hls(format!("Cannot find playlist for segment: {}", segment_url)))
    }

    /// 预取播放列表
    async fn prefetch_playlist(&self, uri: &str) -> Result<(), PluginError> {
        debug!("Starting prefetch for {}", uri);
        let content = Box::pin(self.handle_playlist_internal(uri)).await?;
        debug!("Prefetched {} ({} bytes)", uri, content.len());
        Ok(())
    }

    /// 预取变体流
    pub async fn prefetch_variant(&self, variant: &super::types::Variant) -> Result<(), PluginError> {
        debug!("Prefetching variant: {}", variant.url);
        Box::pin(self.handle_playlist_internal(&variant.url)).await?;
        Ok(())
    }

    /// 预取分片
    pub async fn prefetch_segments(&self, segments: &[super::types::Segment], count: usize) -> Result<(), PluginError> {
        for segment in segments.iter().take(count) {
            if !segment.cached && !segment.downloading {
                debug!("Prefetching segment: {} (duration: {:.2}s)", 
                    segment.url, segment.duration);
                self.segment_manager.handle_segment(&segment.url).await?;
            }
        }
        Ok(())
    }

    /// 处理分片下载
    async fn handle_segment(&self, uri: &str) -> Result<Vec<u8>, PluginError> {
        // 直接使用 SegmentManager
        self.segment_manager.handle_segment(uri).await
    }
}

impl Clone for PlaylistManager {
    fn clone(&self) -> Self {
        Self {
            cache: self.cache.clone(),
            state: self.state.clone(),
            segment_manager: self.segment_manager.clone(),
            _marker: PhantomData,
        }
    }
}

unsafe impl Send for PlaylistManager {}
unsafe impl Sync for PlaylistManager {}

#[derive(Debug)]
pub struct PlaylistStats {
    pub total_playlists: usize,
    pub total_segments: usize,
    pub cached_segments: usize,
    pub downloading_segments: usize,
}