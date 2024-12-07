use crate::bandwidth_control::BandwidthController;
use crate::loader::Loader;
use crate::url_mapper::UrlMapper;
use crate::{error::CacheError, Result};

use futures::StreamExt;
use m3u8_rs::{MediaPlaylist, Playlist};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;
use url::Url;
pub struct HlsHandler {
    loader: Arc<Loader>,
    url_mapper: Arc<UrlMapper>,
    bandwidth_controller: Arc<BandwidthController>,
    current_bandwidth: Arc<AtomicU64>,
}

struct VariantStream {
    bandwidth: u64,
    url: String,
}

#[derive(Clone)]
struct SegmentMergeInfo {
    sequence: u64,
    duration: f32,
    url: String,
}

impl HlsHandler {
    pub fn new(
        loader: Arc<Loader>,
        url_mapper: Arc<UrlMapper>,
        bandwidth_controller: Arc<BandwidthController>,
    ) -> Self {
        Self {
            loader,
            url_mapper,
            bandwidth_controller,
            current_bandwidth: Arc::new(AtomicU64::new(0)),
        }
    }

    pub async fn process_playlist(&self, url: &str) -> Result<Vec<u8>> {
        // 加载 m3u8 内容
        let content = self.loader.load(url).await?;
        let content_str =
            String::from_utf8(content).map_err(|e| CacheError::InvalidInput(e.to_string()))?;

        // 解析 m3u8
        let playlist = m3u8_rs::parse_playlist_res(content_str.as_bytes())
            .map_err(|e| CacheError::InvalidInput(format!("Failed to parse m3u8: {}", e)))?;

        match playlist {
            Playlist::MasterPlaylist(_) => {
                // 主播放列表，直接返回
                Ok(content_str.into_bytes())
            }
            Playlist::MediaPlaylist(mut media_playlist) => {
                // 处理媒体播放列表
                self.process_media_playlist(url, &mut media_playlist).await
            }
        }
    }

    async fn process_media_playlist(
        &self,
        base_url: &str,
        playlist: &mut MediaPlaylist,
    ) -> Result<Vec<u8>> {
        let base = Url::parse(base_url).map_err(|e| CacheError::InvalidInput(e.to_string()))?;

        // 处理每个分片的URL
        for segment in playlist.segments.iter_mut() {
            let uri: &str = segment.uri.as_str();
            let absolute_url = base
                .join(uri)
                .map_err(|e| CacheError::InvalidInput(e.to_string()))?;
            let mapped_url = self.url_mapper.map_url(absolute_url.as_str())?;
            segment.uri = mapped_url;
        }

        // 序列化回 m3u8 格式
        let mut output = Vec::new();
        playlist
            .write_to(&mut output)
            .map_err(|e| CacheError::InvalidInput(format!("Failed to write m3u8: {}", e)))?;

        Ok(output)
    }

    // 添加错误恢复机制
    async fn retry_with_backoff<F, Fut, T>(&self, operation: F, max_retries: u32) -> Result<T>
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = Result<T>>,
    {
        let mut retries = 0;
        let mut delay = Duration::from_millis(100);

        loop {
            match operation().await {
                Ok(result) => return Ok(result),
                Err(e) => {
                    if retries >= max_retries {
                        return Err(e);
                    }
                    retries += 1;
                    tokio::time::sleep(delay).await;
                    delay *= 2; // 指数退避
                }
            }
        }
    }

    // 修改 load_with_bandwidth_control 使用重试机制
    async fn load_with_bandwidth_control(&self, url: &str) -> Result<Vec<u8>> {
        let start = Instant::now();
        let loader = self.loader.clone();
        let url_str = url.to_string();

        let data = self
            .retry_with_backoff(|| async { loader.load(&url_str).await }, 3)
            .await?;

        let duration = start.elapsed();
        self.update_bandwidth_measurement(data.len() as u64, duration)
            .await;
        self.bandwidth_controller.acquire(data.len() as u64).await;

        Ok(data)
    }

    pub async fn preload_segments(&self, url: &str) -> Result<()> {
        let content = self.load_with_bandwidth_control(url).await?;
        let content_str =
            String::from_utf8(content).map_err(|e| CacheError::InvalidInput(e.to_string()))?;

        let playlist = m3u8_rs::parse_playlist_res(content_str.as_bytes())
            .map_err(|e| CacheError::InvalidInput(format!("Failed to parse m3u8: {}", e)))?;

        if let Playlist::MediaPlaylist(media_playlist) = playlist {
            let base = Url::parse(url).map_err(|e| CacheError::InvalidInput(e.to_string()))?;

            let mut tasks = Vec::new();
            for segment in &media_playlist.segments {
                let uri = segment.uri.as_str();
                let absolute_url = base
                    .join(uri)
                    .map_err(|e| CacheError::InvalidInput(e.to_string()))?;
                let loader = self.loader.clone();
                let url_str = absolute_url.to_string();
                tasks.push(tokio::spawn(async move { loader.load(&url_str).await }));
            }

            // 等待所有预加载完成
            for task in tasks {
                task.await.map_err(|e| CacheError::Cache(e.to_string()))??;
            }
        }

        Ok(())
    }

    // 添加新方法用于处理直播流
    pub async fn handle_live_stream(&self, url: &str, max_segments: usize) -> Result<()> {
        let mut current_sequence = 0u64;
        let mut segment_urls = Vec::new();

        loop {
            // 加载最新的播放列表
            let content = self.loader.load(url).await?;
            let content_str =
                String::from_utf8(content).map_err(|e| CacheError::InvalidInput(e.to_string()))?;

            let playlist = m3u8_rs::parse_playlist_res(content_str.as_bytes())
                .map_err(|e| CacheError::InvalidInput(format!("Failed to parse m3u8: {}", e)))?;

            if let Playlist::MediaPlaylist(media_playlist) = playlist {
                let base = Url::parse(url).map_err(|e| CacheError::InvalidInput(e.to_string()))?;

                // 处理新的分片
                for segment in &media_playlist.segments {
                    let uri = segment.uri.as_str();
                    let absolute_url = base
                        .join(uri)
                        .map_err(|e| CacheError::InvalidInput(e.to_string()))?;
                    let url_str = absolute_url.to_string();

                    if !segment_urls.contains(&url_str) {
                        segment_urls.push(url_str.clone());
                        self.load_with_bandwidth_control(&url_str).await?;
                    }
                }

                // 限制缓存的分片数量
                if segment_urls.len() > max_segments {
                    segment_urls.drain(0..segment_urls.len() - max_segments);
                }

                // 更新序列号
                if media_playlist.media_sequence > current_sequence {
                    current_sequence = media_playlist.media_sequence;
                }
            }

            // 等待下一个更新周期
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }

    // 添加带宽自适应支持
    pub async fn handle_adaptive_stream(&self, master_url: &str) -> Result<Vec<String>> {
        let content = self.loader.load(master_url).await?;
        let content_str =
            String::from_utf8(content).map_err(|e| CacheError::InvalidInput(e.to_string()))?;

        let playlist = m3u8_rs::parse_playlist_res(content_str.as_bytes())
            .map_err(|e| CacheError::InvalidInput(format!("Failed to parse m3u8: {}", e)))?;

        let mut variant_urls = Vec::new();

        if let Playlist::MasterPlaylist(master) = playlist {
            let base =
                Url::parse(master_url).map_err(|e| CacheError::InvalidInput(e.to_string()))?;

            // 收集所有码率的播放列表URL
            for variant in &master.variants {
                let uri = variant.uri.as_str();
                let absolute_url = base
                    .join(uri)
                    .map_err(|e| CacheError::InvalidInput(e.to_string()))?;
                variant_urls.push(absolute_url.to_string());
            }
        }

        Ok(variant_urls)
    }

    // 添加带宽自适应选择功能
    pub async fn select_variant(&self, master_url: &str) -> Result<String> {
        let variants = self.handle_adaptive_stream(master_url).await?;
        let mut streams = Vec::new();

        // 解析每个变体流的带宽信息
        for variant_url in variants {
            let content = self.load_with_bandwidth_control(&variant_url).await?;
            let content_str =
                String::from_utf8(content).map_err(|e| CacheError::InvalidInput(e.to_string()))?;

            let playlist = m3u8_rs::parse_playlist_res(content_str.as_bytes())
                .map_err(|e| CacheError::InvalidInput(format!("Failed to parse m3u8: {}", e)))?;

            if let Playlist::MediaPlaylist(media_playlist) = playlist {
                let bandwidth = media_playlist.target_duration;
                streams.push(VariantStream {
                    bandwidth: bandwidth as u64 * 1024, // 转换为 bps
                    url: variant_url,
                });
            }
        }

        // 根据当前带宽选择合适的流
        let current_bw = self.current_bandwidth.load(Ordering::Relaxed);
        streams.sort_by_key(|s| s.bandwidth);

        let selected = streams
            .iter()
            .rev() // 从高到低历
            .find(|s| s.bandwidth <= current_bw * 8 / 10) // 使用 80% 带宽阈值
            .or_else(|| streams.first()) // 如果没有合适的，选择最低码率
            .ok_or_else(|| {
                CacheError::InvalidInput("No valid variant streams found".to_string())
            })?;

        Ok(selected.url.clone())
    }

    // 添加自适应流处理功能
    pub async fn handle_adaptive_playback(&self, master_url: &str) -> Result<()> {
        let mut current_url = self.select_variant(master_url).await?;
        let mut last_switch = Instant::now();

        loop {
            // 处理当前选中的流
            self.handle_live_stream(&current_url, 3).await?;

            // 每 30 秒检查一次是否需要切换码率
            if last_switch.elapsed() >= Duration::from_secs(30) {
                if let Ok(new_url) = self.select_variant(master_url).await {
                    if new_url != current_url {
                        current_url = new_url;
                        last_switch = Instant::now();
                    }
                }
            }

            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    // 更新带宽测量
    async fn update_bandwidth_measurement(&self, bytes: u64, duration: Duration) {
        if duration.as_secs() > 0 {
            let bps = bytes * 8 / duration.as_secs();
            self.current_bandwidth.store(bps, Ordering::Relaxed);
        }
    }

    // 添加分片合并功能
    async fn merge_segments(&self, segments: &[SegmentMergeInfo]) -> Result<Vec<u8>> {
        let mut merged_data = Vec::new();
        let mut current_sequence = segments[0].sequence;

        for segment in segments {
            // 确保分片连续
            if segment.sequence != current_sequence {
                return Err(CacheError::InvalidInput(
                    "Discontinuous segments".to_string(),
                ));
            }

            // 加载分片数据
            let data = self.load_with_bandwidth_control(&segment.url).await?;
            merged_data.extend_from_slice(&data);
            current_sequence += 1;
        }

        Ok(merged_data)
    }

    // 优化分片预加载
    pub async fn preload_segments_optimized(
        &self,
        url: &str,
        merge_threshold: usize,
    ) -> Result<()> {
        let content = self.load_with_bandwidth_control(url).await?;
        let content_str =
            String::from_utf8(content).map_err(|e| CacheError::InvalidInput(e.to_string()))?;

        let playlist = m3u8_rs::parse_playlist_res(content_str.as_bytes())
            .map_err(|e| CacheError::InvalidInput(format!("Failed to parse m3u8: {}", e)))?;

        if let Playlist::MediaPlaylist(media_playlist) = playlist {
            let base = Url::parse(url).map_err(|e| CacheError::InvalidInput(e.to_string()))?;

            // 收集分片信息
            let mut segments: Vec<Vec<SegmentMergeInfo>> = Vec::new();
            let mut current_batch: Vec<SegmentMergeInfo> = Vec::new();
            let mut sequence = media_playlist.media_sequence;

            for segment in &media_playlist.segments {
                let uri = segment.uri.as_str();
                let absolute_url = base
                    .join(uri)
                    .map_err(|e| CacheError::InvalidInput(e.to_string()))?;

                current_batch.push(SegmentMergeInfo {
                    sequence,
                    duration: segment.duration,
                    url: absolute_url.to_string(),
                });

                // 当达到合并阈值时处理批次
                if current_batch.len() >= merge_threshold {
                    segments.push(current_batch.clone());
                    current_batch.clear();
                }

                sequence += 1;
            }

            // 处理剩余的分片
            if !current_batch.is_empty() {
                segments.push(current_batch);
            }

            // 异步处理每个批次
            let stream = futures::stream::iter(segments)
                .map(|batch| async move { self.merge_segments(&batch).await });

            // 等待所有批次处理完成
            stream
                .for_each_concurrent(None, |task| async {
                    let _ = task.await;
                })
                .await;
        }

        Ok(())
    }

    // 添加分片合并的缓存管理
    pub async fn handle_live_stream_optimized(
        &self,
        url: &str,
        max_segments: usize,
        merge_threshold: usize,
    ) -> Result<()> {
        let mut current_sequence = 0u64;
        let mut merged_segments = Vec::new();
        let mut current_batch = Vec::new();

        loop {
            let content = self.load_with_bandwidth_control(url).await?;
            let content_str =
                String::from_utf8(content).map_err(|e| CacheError::InvalidInput(e.to_string()))?;

            let playlist = m3u8_rs::parse_playlist_res(content_str.as_bytes())
                .map_err(|e| CacheError::InvalidInput(format!("Failed to parse m3u8: {}", e)))?;

            if let Playlist::MediaPlaylist(media_playlist) = playlist {
                let base = Url::parse(url).map_err(|e| CacheError::InvalidInput(e.to_string()))?;

                for segment in &media_playlist.segments {
                    let uri = segment.uri.as_str();
                    let absolute_url = base
                        .join(uri)
                        .map_err(|e| CacheError::InvalidInput(e.to_string()))?;

                    let segment_info = SegmentMergeInfo {
                        sequence: current_sequence,
                        duration: segment.duration,
                        url: absolute_url.to_string(),
                    };

                    current_batch.push(segment_info);

                    // 当达到合并阈值时处理批次
                    if current_batch.len() >= merge_threshold {
                        let merged_data = self.merge_segments(&current_batch).await?;
                        merged_segments.push(merged_data);
                        current_batch.clear();

                        // 限制合并后的分片数量
                        if merged_segments.len() > max_segments {
                            merged_segments.remove(0);
                        }
                    }

                    current_sequence += 1;
                }
            }

            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }

    // 添加缓存预热功能
    pub async fn warmup_cache(&self, master_url: &str, depth: usize) -> Result<()> {
        // 预热主播放列表
        let variants = self.handle_adaptive_stream(master_url).await?;

        // 并发预热所有码率的播放列表
        let stream = futures::stream::iter(variants).map(|variant_url| async move {
            // 加载播放列表
            let content = self.load_with_bandwidth_control(&variant_url).await?;
            let content_str =
                String::from_utf8(content).map_err(|e| CacheError::InvalidInput(e.to_string()))?;

            let playlist = m3u8_rs::parse_playlist_res(content_str.as_bytes())
                .map_err(|e| CacheError::InvalidInput(format!("Failed to parse m3u8: {}", e)))?;

            if let Playlist::MediaPlaylist(media_playlist) = playlist {
                // 只预热指定深度的分片
                let segments = media_playlist
                    .segments
                    .iter()
                    .take(depth)
                    .filter_map(|segment| Some(segment.uri.as_str()))
                    .collect::<Vec<_>>();

                // 并发预热分片
                let base = Url::parse(&variant_url)
                    .map_err(|e| CacheError::InvalidInput(e.to_string()))?;
                for uri in segments {
                    let absolute_url = base
                        .join(uri)
                        .map_err(|e| CacheError::InvalidInput(e.to_string()))?;
                    self.load_with_bandwidth_control(absolute_url.as_str())
                        .await?;
                }
            }
            Ok::<_, CacheError>(())
        });

        // 等待所有预热任务完成
        stream
            .buffer_unordered(4) // 限制并发数
            .for_each(|result| async {
                if let Err(e) = result {
                    eprintln!("Warmup error: {}", e);
                }
            })
            .await;

        Ok(())
    }

    // 添加智能预加载功能
    pub async fn smart_preload(&self, url: &str, max_depth: usize) -> Result<()> {
        let content = self
            .retry_with_backoff(|| async { self.load_with_bandwidth_control(url).await }, 3)
            .await?;

        let content_str =
            String::from_utf8(content).map_err(|e| CacheError::InvalidInput(e.to_string()))?;

        let playlist = m3u8_rs::parse_playlist_res(content_str.as_bytes())
            .map_err(|e| CacheError::InvalidInput(format!("Failed to parse m3u8: {}", e)))?;

        if let Playlist::MediaPlaylist(media_playlist) = playlist {
            let base = Url::parse(url).map_err(|e| CacheError::InvalidInput(e.to_string()))?;

            // 智能选择预加载深度
            let depth = if media_playlist.segments.len() < max_depth {
                media_playlist.segments.len()
            } else {
                max_depth
            };

            // 并发预加载选定的分片
            let mut url_strs = Vec::new();
            for segment in media_playlist.segments.iter().take(depth) {
                let uri = segment.uri.as_str();
                let absolute_url = base
                    .join(uri)
                    .map_err(|e| CacheError::InvalidInput(e.to_string()))?;
                let url_str = absolute_url.to_string();
                url_strs.push(url_str);
            }

            // 异步处理每个批次
            let stream = futures::stream::iter(url_strs)
                .map(|url_str| async move { self.load_with_bandwidth_control(&url_str).await });

            // 等待所有批次处理完成
            stream
                .for_each_concurrent(None, |task| async {
                    let _ = task.await;
                })
                .await;
        }

        Ok(())
    }
}

// 为 HlsHandler 实现 Clone
impl Clone for HlsHandler {
    fn clone(&self) -> Self {
        Self {
            loader: self.loader.clone(),
            url_mapper: self.url_mapper.clone(),
            bandwidth_controller: self.bandwidth_controller.clone(),
            current_bandwidth: self.current_bandwidth.clone(),
        }
    }
}
