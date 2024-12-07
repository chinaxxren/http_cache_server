use std::sync::Arc;
use tokio::sync::Semaphore;
use tokio::time::{Duration, Instant};
use std::sync::atomic::{AtomicU64, Ordering};

pub struct BandwidthLimiter {
    bytes_per_second: AtomicU64,
    token_bucket: Arc<Semaphore>,
    last_refill: Arc<parking_lot::Mutex<Instant>>,
}

impl BandwidthLimiter {
    pub fn new(bytes_per_second: u64) -> Self {
        Self {
            bytes_per_second: AtomicU64::new(bytes_per_second),
            token_bucket: Arc::new(Semaphore::new(bytes_per_second as usize)),
            last_refill: Arc::new(parking_lot::Mutex::new(Instant::now())),
        }
    }

    pub fn set_bandwidth_limit(&self, bytes_per_second: u64) {
        self.bytes_per_second.store(bytes_per_second, Ordering::Relaxed);
    }

    pub async fn acquire_tokens(&self, bytes: usize) {
        let mut remaining = bytes;
        while remaining > 0 {
            self.refill_tokens().await;
            let to_acquire = remaining.min(self.bytes_per_second.load(Ordering::Relaxed) as usize);
            let permit = self.token_bucket.acquire_many(to_acquire as u32).await.unwrap();
            permit.forget();
            remaining -= to_acquire;
        }
    }

    async fn refill_tokens(&self) {
        let mut last_refill = self.last_refill.lock();
        let now = Instant::now();
        let elapsed = now.duration_since(*last_refill);
        
        if elapsed >= Duration::from_secs(1) {
            *last_refill = now;
            let bytes_per_second = self.bytes_per_second.load(Ordering::Relaxed) as usize;
            self.token_bucket.add_permits(bytes_per_second);
        }
    }
} 