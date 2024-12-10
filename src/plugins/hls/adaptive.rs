use std::collections::HashMap;
use tokio::sync::RwLock;
use std::sync::Arc;
use tracing::{info, debug};
use serde::{Serialize, Deserialize};
use super::types::Variant;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bandwidth {
    pub current: u64,
    pub min: u64,
    pub max: u64,
    pub average: u64,
}

#[derive(Debug)]
pub struct AdaptiveStream {
    bandwidth: Arc<RwLock<Bandwidth>>,
    window_size: usize,
    samples: Vec<u64>,
}

impl AdaptiveStream {
    pub fn new(initial_bandwidth: u64) -> Self {
        Self {
            bandwidth: Arc::new(RwLock::new(Bandwidth {
                current: initial_bandwidth,
                min: initial_bandwidth,
                max: initial_bandwidth,
                average: initial_bandwidth,
            })),
            window_size: 10,
            samples: Vec::new(),
        }
    }

    pub async fn update_bandwidth(&mut self, bytes: u64, duration_ms: u64) {
        let bps = (bytes * 8 * 1000) / duration_ms;
        self.samples.push(bps);

        if self.samples.len() > self.window_size {
            self.samples.remove(0);
        }

        let mut bandwidth = self.bandwidth.write().await;
        bandwidth.current = bps;
        bandwidth.min = *self.samples.iter().min().unwrap_or(&bps);
        bandwidth.max = *self.samples.iter().max().unwrap_or(&bps);
        bandwidth.average = self.samples.iter().sum::<u64>() / self.samples.len() as u64;
    }

    pub async fn select_variant<'a>(&self, variants: &'a [super::types::Variant]) -> Option<&'a super::types::Variant> {
        let bandwidth = self.bandwidth.read().await;
        let target = bandwidth.average;

        variants.iter()
            .rev()
            .find(|v| v.bandwidth <= target * 8 / 10)
            .or_else(|| variants.first())
    }
}

#[derive(Debug)]
pub struct AdaptiveStreamManager {
    streams: Arc<RwLock<HashMap<String, StreamState>>>,
}

#[derive(Debug)]
struct StreamState {
    bandwidth: f64,
    quality_levels: Vec<QualityLevel>,
}

#[derive(Debug)]
struct QualityLevel {
    bandwidth: f64,
    resolution: (u32, u32),
}

impl AdaptiveStreamManager {
    pub fn new() -> Self {
        Self {
            streams: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn register_stream(&self, stream_id: String, initial_bandwidth: f64) {
        debug!(
            "Registering new stream {} with initial bandwidth {} bps",
            stream_id, initial_bandwidth
        );
        let mut streams = self.streams.write().await;
        streams.insert(stream_id.clone(), StreamState {
            bandwidth: initial_bandwidth,
            quality_levels: vec![],
        });
        info!("Stream {} registered successfully", stream_id);
    }

    pub async fn update_bandwidth(&self, stream_id: &str, bandwidth: f64) {
        let mut streams = self.streams.write().await;
        if let Some(state) = streams.get_mut(stream_id) {
            const ALPHA: f64 = 0.3;
            state.bandwidth = state.bandwidth * (1.0 - ALPHA) + bandwidth * ALPHA;
            debug!(
                "Updated bandwidth for stream {}: {:.2} Mbps (instant: {:.2} Mbps)",
                stream_id,
                state.bandwidth / 1_000_000.0,
                bandwidth / 1_000_000.0
            );
        }
    }

    pub async fn get_bandwidth(&self, stream_id: &str) -> f64 {
        let streams = self.streams.read().await;
        streams.get(stream_id)
            .map(|state| state.bandwidth)
            .unwrap_or(1_000_000.0) // 默认 1Mbps
    }

    pub async fn get_optimal_variant<'a>(&self, stream_id: &str, variants: &'a [Variant]) -> Option<&'a Variant> {
        let bandwidth = self.get_bandwidth(stream_id).await;
        let target = bandwidth * 0.8;

        variants.iter()
            .rev()
            .find(|v| v.bandwidth as f64 <= target)
            .or_else(|| variants.first())
    }
} 