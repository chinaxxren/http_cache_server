use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use crate::{Result, error::CacheError};
use crate::storage::Storage;
use dashmap::DashMap;

// 访问统计信息
#[derive(Clone)]
struct AccessStats {
    count: u64,
    last_access: Instant,
    pattern: AccessPattern,
    access_history: VecDeque<(u32, Instant)>, // (segment_index, access_time)
}

#[derive(Clone, Copy, PartialEq)]
enum AccessPattern {
    Sequential,  // 顺序访问
    Random,      // 随机访问
    Periodic,    // 周期性访问
    Burst,       // 突发访问
}

pub struct OptimizerConfig {
    max_history: usize,
    merge_threshold: usize,
    hot_threshold: f64,
    decay_factor: f64,
}

impl Default for OptimizerConfig {
    fn default() -> Self {
        Self {
            max_history: 100,
            merge_threshold: 1024 * 1024 * 5,
            hot_threshold: 5.0,
            decay_factor: 0.95,
        }
    }
}

pub struct PerformanceOptimizer {
    storage: Arc<Storage>,
    stats: Arc<DashMap<String, AccessStats>>,
    hot_content: Arc<DashMap<String, f64>>,
    config: OptimizerConfig,
    preload_queue: Arc<RwLock<VecDeque<(String, u32)>>>, // (key, segment_index)
}

impl PerformanceOptimizer {
    pub fn new(storage: Arc<Storage>, config: OptimizerConfig) -> Self {
        Self {
            storage,
            stats: Arc::new(DashMap::new()),
            hot_content: Arc::new(DashMap::new()),
            config,
            preload_queue: Arc::new(RwLock::new(VecDeque::new())),
        }
    }

    // 改进的访问模式检测
    async fn detect_pattern(&self, key: &str, current_index: u32) -> AccessPattern {
        if let Some(stats) = self.stats.get(key) {
            let history = &stats.access_history;
            if history.len() < 3 {
                return AccessPattern::Random;
            }

            // 检查顺序访问
            let mut is_sequential = true;
            let mut prev_index = None;
            for (index, _) in history.iter() {
                if let Some(prev) = prev_index {
                    if *index != prev + 1 {
                        is_sequential = false;
                        break;
                    }
                }
                prev_index = Some(*index);
            }
            if is_sequential {
                return AccessPattern::Sequential;
            }

            // 检查周期性访问
            let mut intervals = Vec::new();
            let mut prev_time = None;
            for (_, time) in history.iter() {
                if let Some(prev) = prev_time {
                    intervals.push(time.duration_since(prev));
                }
                prev_time = Some(*time);
            }
            
            let avg_interval = intervals.iter().sum::<Duration>() / intervals.len() as u32;
            let variance: f64 = intervals.iter()
                .map(|i| {
                    let diff = i.as_secs_f64() - avg_interval.as_secs_f64();
                    diff * diff
                })
                .sum::<f64>() / intervals.len() as f64;

            if variance < 1.0 {
                return AccessPattern::Periodic;
            }

            // 检查突发访问
            let recent_count = history.iter()
                .filter(|(_, time)| time.elapsed() < Duration::from_secs(10))
                .count();
            if recent_count > 5 {
                return AccessPattern::Burst;
            }
        }
        AccessPattern::Random
    }

    // 优化的分片合并策略
    pub async fn optimize_merge_strategy(&self, key: &str, total_segments: u32) -> Result<Vec<Vec<u32>>> {
        let mut merged_groups = Vec::new();
        let mut current_group = Vec::new();
        let mut current_size = 0u64;
        let pattern = self.detect_pattern(key, 0).await;

        let max_group_size = match pattern {
            AccessPattern::Sequential => 1024 * 1024 * 10, // 10MB
            AccessPattern::Periodic => 1024 * 1024 * 5,    // 5MB
            AccessPattern::Burst => 1024 * 1024 * 2,      // 2MB
            AccessPattern::Random => 1024 * 1024,         // 1MB
        };

        for i in 0..total_segments {
            if let Some(chunk_size) = self.get_chunk_size(key, i).await? {
                if current_size + chunk_size > max_group_size {
                    if !current_group.is_empty() {
                        merged_groups.push(current_group);
                        current_group = Vec::new();
                        current_size = 0;
                    }
                }
                current_group.push(i);
                current_size += chunk_size;
            }
        }

        if !current_group.is_empty() {
            merged_groups.push(current_group);
        }

        Ok(merged_groups)
    }

    // 预加载队列管理
    pub async fn add_to_preload_queue(&self, key: String, segment_index: u32) {
        let mut queue = self.preload_queue.write().await;
        queue.push_back((key, segment_index));
        if queue.len() > self.config.max_history {
            queue.pop_front();
        }
    }

    pub async fn process_preload_queue(&self) -> Result<()> {
        let batch_size = 10;
        let mut futures = Vec::with_capacity(batch_size);
        
        let mut queue = self.preload_queue.write().await;
        while let Some((key, index)) = queue.pop_front() {
            if self.is_hot_content(&key).await {
                // 创建一个新的 Future，将 key 移动到其中
                let storage = self.storage.clone();
                let future = async move {
                    storage.read_chunk(&key, index).await
                };
                futures.push(future);
                
                if futures.len() >= batch_size {
                    futures::future::join_all(futures).await;
                    futures = Vec::with_capacity(batch_size);
                }
            }
        }
        Ok(())
    }

    // 预测性缓存
    pub async fn predict_and_cache(&self, key: &str, segment_index: u32) -> Result<()> {
        if let Some(stats) = self.stats.get(key) {
            match stats.pattern {
                AccessPattern::Sequential => {
                    // 预加载后续的几个分片
                    for i in 1..=3 {
                        let next_index = segment_index + i;
                        let _ = self.storage.read_chunk(key, next_index).await;
                    }
                }
                AccessPattern::Periodic => {
                    // 预加载周期性出现的分片
                    if let Some(next_time) = self.predict_next_access(key).await {
                        if next_time < Duration::from_secs(60) {
                            let _ = self.storage.read_chunk(key, segment_index).await;
                        }
                    }
                }
                AccessPattern::Random => {
                    // 对于随机访问，只预加载热点内容
                    if self.is_hot_content(key).await {
                        let _ = self.storage.read_chunk(key, segment_index).await;
                    }
                }
                AccessPattern::Burst => {
                    // 对于突发访问，预加载当前和后续的分片
                    let _ = self.storage.read_chunk(key, segment_index).await;
                    for i in 1..=3 {
                        let next_index = segment_index + i;
                        let _ = self.storage.read_chunk(key, next_index).await;
                    }
                }
            }
        }
        Ok(())
    }

    // 热点内容优化
    pub async fn update_hot_content(&self, key: &str) {
        if let Some(mut entry) = self.hot_content.get_mut(key) {
            let new_score = *entry * 0.95 + 1.0;
            *entry = new_score;
        } else {
            self.hot_content.insert(key.to_string(), 1.0);
        }

        // 清理低于阈值的内容
        self.hot_content.retain(|_, score| *score > 0.1);
    }

    async fn is_hot_content(&self, key: &str) -> bool {
        self.hot_content.get(key)
            .map_or(false, |score| *score > self.config.hot_threshold)
    }

    async fn get_chunk_size(&self, key: &str, index: u32) -> Result<Option<u64>> {
        if let Some(data) = self.storage.read_chunk(key, index).await? {
            Ok(Some(data.len() as u64))
        } else {
            Ok(None)
        }
    }

    // 预测下次访问时间
    async fn predict_next_access(&self, key: &str) -> Option<Duration> {
        self.stats.get(key).map(|stats| {
            stats.last_access.elapsed()
        })
    }

    // 更新访问统计
    pub async fn record_access(&self, key: &str, segment_index: u32) {
        let mut entry = self.stats.entry(key.to_string()).or_insert(AccessStats {
            count: 0,
            last_access: Instant::now(),
            pattern: AccessPattern::Random,
            access_history: VecDeque::with_capacity(100),
        });

        entry.count += 1;
        entry.last_access = Instant::now();
        entry.access_history.push_back((segment_index, Instant::now()));
        if entry.access_history.len() > self.config.max_history {
            entry.access_history.pop_front();
        }

        // 更新访问模式
        if entry.count > 5 {
            entry.pattern = self.detect_pattern(key, segment_index).await;
        }

        // 更新热点内容
        self.update_hot_content(key).await;
    }
} 