use std::sync::Arc;
use tokio::sync::RwLock;
use std::collections::HashMap;
use tracing::debug;

#[derive(Debug)]
pub struct SegmentManager {
    segments: Arc<RwLock<HashMap<String, u64>>>,
}

impl SegmentManager {
    pub fn new() -> Self {
        Self {
            segments: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn add_segment(&self, segment_id: String, size: u64) {
        let mut segments = self.segments.write().await;
        segments.insert(segment_id, size);
        debug!("Added segment: {} bytes", size);
    }

    pub async fn get_total_size(&self) -> u64 {
        let segments = self.segments.read().await;
        segments.values().sum()
    }
}