use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use std::collections::HashMap;

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
        Self {
            cache_dir: cache_dir.as_ref().to_owned(),
            segments: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn add_segment(&self, segment_id: String, size: u64) {
        let path = self.cache_dir.join(&segment_id);
        let mut segments = self.segments.write().await;
        segments.insert(segment_id, SegmentInfo {
            _path: path,
            _size: size,
            _last_access: std::time::Instant::now(),
        });
    }
}