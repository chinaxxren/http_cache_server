use std::sync::Arc;
use tokio::sync::RwLock;
use std::collections::HashMap;
use std::time::Instant;
use crate::error::PluginError;

#[derive(Debug)]
pub struct BandwidthManager {
    limits: Arc<RwLock<HashMap<String, BandwidthLimit>>>,
}

#[derive(Debug)]
struct BandwidthLimit {
    rate: f64,  // bytes per second
    last_update: Instant,
    bytes_transferred: u64,
}

impl BandwidthManager {
    pub fn new() -> Self {
        Self {
            limits: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    #[allow(dead_code)]
    pub async fn set_limit(&self, stream_id: &str, rate: f64) {
        let mut limits = self.limits.write().await;
        limits.insert(stream_id.to_string(), BandwidthLimit {
            rate,
            last_update: Instant::now(),
            bytes_transferred: 0,
        });
    }

    pub async fn check_limit(&self, stream_id: &str, bytes: u64) -> Result<(), PluginError> {
        let mut limits = self.limits.write().await;
        if let Some(limit) = limits.get_mut(stream_id) {
            let now = Instant::now();
            let elapsed = now.duration_since(limit.last_update).as_secs_f64();
            
            if elapsed > 0.0 {
                let current_rate = limit.bytes_transferred as f64 / elapsed;
                if current_rate > limit.rate {
                    return Err(PluginError::Network("Bandwidth limit exceeded".into()));
                }
            }
            
            limit.bytes_transferred += bytes;
            limit.last_update = now;
        }
        Ok(())
    }
} 