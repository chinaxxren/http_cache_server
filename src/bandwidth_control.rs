use std::sync::Arc;
use tokio::sync::Semaphore;
use std::time::{Duration, Instant};
use std::sync::atomic::{AtomicU64, Ordering};
use parking_lot::Mutex;

pub struct BandwidthController {
    bytes_per_second: AtomicU64,
    current_window: Arc<Mutex<(Instant, u64)>>,
    limiter: Arc<Semaphore>,
}

impl BandwidthController {
    pub fn new(bytes_per_second: u64) -> Self {
        Self {
            bytes_per_second: AtomicU64::new(bytes_per_second),
            current_window: Arc::new(Mutex::new((Instant::now(), 0))),
            limiter: Arc::new(Semaphore::new(1)),
        }
    }

    pub async fn acquire(&self, bytes: u64) {
        let _permit = self.limiter.acquire().await.unwrap();
        let now = Instant::now();
        let mut window = self.current_window.lock();

        if now.duration_since(window.0) >= Duration::from_secs(1) {
            // Reset window
            *window = (now, 0);
        }

        let target_duration = Duration::from_secs_f64(
            bytes as f64 / self.bytes_per_second.load(Ordering::Relaxed) as f64
        );

        window.1 += bytes;
        if window.1 > self.bytes_per_second.load(Ordering::Relaxed) {
            let sleep_duration = target_duration.saturating_sub(
                Duration::from_secs(1) - now.duration_since(window.0)
            );
            drop(window);
            tokio::time::sleep(sleep_duration).await;
        }
    }

    pub fn set_bandwidth_limit(&self, bytes_per_second: u64) {
        self.bytes_per_second.store(bytes_per_second, Ordering::Relaxed);
    }
} 