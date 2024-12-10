use std::sync::Arc;
use hyper::body::Sender;
use tokio::sync::RwLock;
use std::collections::HashMap;
use tracing::{debug, info, warn};
use crate::error::PluginError;
use tokio::io::AsyncWriteExt;

#[derive(Debug)]
pub struct SegmentManager {
    segments: Arc<RwLock<HashMap<String, SegmentInfo>>>,
}

#[derive(Debug)]
struct SegmentInfo {
    size: u64,
    downloading: bool,
    bytes_downloaded: u64,
}

impl SegmentManager {
    pub fn new() -> Self {
        Self {
            segments: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn handle_segment(&self, uri: &str) -> Result<Vec<u8>, PluginError> {
        // 从 URL 中提取播放列表路径和分片名
        let playlist_url = super::utils::get_playlist_url(uri);
        let playlist_hash = super::utils::hash_url(&playlist_url);
        let segment_name = super::utils::get_segment_name(uri);

        // 构建存储路径
        let base_path = std::path::PathBuf::from("cache/hls").join(&playlist_hash);
        let file_path = base_path.join("segments").join(segment_name);

        // 确保目录存在
        tokio::fs::create_dir_all(base_path.join("segments")).await?;

        // 尝试从文件系统读取
        if let Ok(content) = tokio::fs::read(&file_path).await {
            debug!("File system hit for segment {}", uri);
            return Ok(content);
        }

        // 下载并存储
        debug!("File system miss for segment {}, downloading...", uri);
        let content = self.download_segment(uri).await?;

        // 确保写入成功
        tokio::fs::write(&file_path, &content).await?;
        debug!("Segment {} saved to {}", uri, file_path.display());

        Ok(content)
    }

    async fn download_segment(&self, uri: &str) -> Result<Vec<u8>, PluginError> {
        let resp = self.download_with_retry(uri, 3).await?;
        let status = resp.status();
        if !status.is_success() {
            return Err(PluginError::Network(format!("Server returned status: {}", status)));
        }

        let body = hyper::body::to_bytes(resp.into_body())
            .await
            .map_err(|e| PluginError::Network(e.to_string()))?;

        let content = body.to_vec();

        // 更新状态
        let mut segments = self.segments.write().await;
        segments.insert(uri.to_string(), SegmentInfo {
            size: content.len() as u64,
            downloading: false,
            bytes_downloaded: content.len() as u64,
        });

        Ok(content)
    }

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

    pub async fn stream_segment(&self, uri: &str, mut sender: Sender) -> Result<(), PluginError> {
        // 从 URL 中提取播放列表路径和分片名
        let playlist_url = super::utils::get_playlist_url(uri);
        let playlist_hash = super::utils::hash_url(&playlist_url);
        let segment_name = super::utils::get_segment_name(uri);

        // 构建存储路径
        let base_path = std::path::PathBuf::from("cache/hls").join(&playlist_hash);
        let file_path = base_path.join("segments").join(segment_name);

        // 确保目录存在
        tokio::fs::create_dir_all(base_path.join("segments")).await?;

        // 尝试从文件系统读取
        if let Ok(content) = tokio::fs::read(&file_path).await {
            debug!("File system hit for segment {}", uri);
            sender.send_data(bytes::Bytes::from(content)).await
                .map_err(|e| PluginError::Network(e.to_string()))?;
            return Ok(());
        }

        // 流式下载
        debug!("File system miss for segment {}, streaming...", uri);
        self.stream_download(uri, sender).await
    }

    async fn stream_download(&self, uri: &str, mut sender: Sender) -> Result<(), PluginError> {
        let resp = self.download_with_retry(uri, 3).await?;
        let status = resp.status();
        if !status.is_success() {
            return Err(PluginError::Network(format!("Server returned status: {}", status)));
        }

        let hash = super::utils::hash_url(uri);
        let base_path = std::path::PathBuf::from("cache/hls").join(&hash);
        
        // 确保目录存在
        tokio::fs::create_dir_all(&base_path).await?;

        let file_path = base_path.join("segment.ts");

        // 创建文件
        let mut file = tokio::fs::File::create(&file_path).await?;

        let mut body = resp.into_body();
        let mut total_bytes = 0;
        let start_time = std::time::Instant::now();

        while let Some(chunk) = hyper::body::HttpBody::data(&mut body).await {
            let chunk = chunk.map_err(|e| PluginError::Network(e.to_string()))?;
            let chunk_size = chunk.len();
            total_bytes += chunk_size;

            // 写入文件
            file.write_all(&chunk).await?;

            // 发送给客户端
            sender.send_data(bytes::Bytes::from(chunk)).await
                .map_err(|e| PluginError::Network(format!("Failed to send chunk: {}", e)))?;
        }

        // 确保文件写入完成
        file.flush().await?;

        let elapsed = start_time.elapsed();
        info!(
            "Segment {} streamed and saved: {:.2} MB in {:.2}s",
            uri,
            total_bytes as f64 / (1024.0 * 1024.0),
            elapsed.as_secs_f64()
        );

        // 更新状态
        let mut segments = self.segments.write().await;
        segments.insert(uri.to_string(), SegmentInfo {
            size: total_bytes as u64,
            downloading: false,
            bytes_downloaded: total_bytes as u64,
        });

        Ok(())
    }
}