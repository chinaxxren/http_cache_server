use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use std::collections::HashMap;
use tracing::{info, debug};

#[derive(Debug)]
pub struct SegmentManager {
    cache_dir: PathBuf,
    segments: Arc<RwLock<HashMap<String, SegmentInfo>>>,
}

#[derive(Debug)]
struct SegmentInfo {
    _path: PathBuf,
    _size: u64,
    _last_access: std::time::Instant,
}

impl SegmentManager {
    pub fn new<P: AsRef<Path>>(cache_dir: P) -> Self {
        debug!("Initializing SegmentManager with cache dir: {:?}", cache_dir.as_ref());
        Self {
            cache_dir: cache_dir.as_ref().to_owned(),
            segments: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn add_segment(&self, segment_id: String, size: u64) {
        debug!("Adding segment {} ({} bytes) to cache", segment_id, size);
        let path = self.cache_dir.join(&segment_id);
        let mut segments = self.segments.write().await;
        segments.insert(segment_id.clone(), SegmentInfo {
            _path: path.clone(),
            _size: size,
            _last_access: std::time::Instant::now(),
        });
        info!("Segment {} added to cache at {:?}", segment_id, path);
    }

    pub async fn remove_segment(&self, segment_id: &str) -> Result<(), std::io::Error> {
        debug!("Removing segment {} from cache", segment_id);
        let mut segments = self.segments.write().await;
        if let Some(info) = segments.remove(segment_id) {
            if let Err(e) = tokio::fs::remove_file(&info._path).await {
                info!("Failed to remove segment file: {}", e);
                return Err(e);
            }
            info!("Segment {} removed from cache", segment_id);
        } else {
            debug!("Segment {} not found in cache", segment_id);
        }
        Ok(())
    }

    pub async fn get_segment_count(&self) -> usize {
        let segments = self.segments.read().await;
        let count = segments.len();
        debug!("Current segment count: {}", count);
        count
    }

    pub async fn get_total_size(&self) -> u64 {
        let segments = self.segments.read().await;
        let total = segments.values().map(|info| info._size).sum();
        debug!("Total cache size: {} bytes", total);
        total
    }
}