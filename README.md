# HTTP Cache Server

一个高性能的 HTTP 缓存服务器，专门针对 HLS 流媒体进行了优化。支持智能缓存管理、带宽控制和性能优化。

## 主要特性

### 1. HLS 流媒体支持
- 播放列表解析和处理
- 分片缓存和预加载
- 多码率自适应支持
- 直播流处理
- 智能分片合并

### 2. 缓存管理
- 基于时间的过期清理
- 基于容量的 LRU 清理
- 基于访问频率的清理
- 智能缓存预热
- 分片存储优化

### 3. 性能优化
- 预测性缓存
  - 顺序访问优化
  - 周期性访问识别
  - 随机访问处理
- 热点内容优化
  - 动态热度计算
  - 自适应缓存策略
- 分片合并策略
  - 智能分组
  - 大小限制
  - 访问模式适应

### 4. 带宽控制
- 动态带宽限制
- 令牌桶算法
- 自适应码率选择
- 带宽监控和测量

### 5. 资源管理
- 并发下载控制
- 内存使用优化
- 磁盘空间管理
- 资源清理策略

## 技术栈
- Rust
- Tokio 异步运行时
- Hyper HTTP 服务器
- M3U8 解析器
- 异步 I/O

## 系统要求
- Rust 1.70 或更高版本
- 支持异步 I/O 的操作系统
- 足够的磁盘空间用于缓存

## 配置项
- 最大并发下载数
- 缓存容量限制
- 带宽限制
- 分片合并阈值
- 清理周期
- 预加载深度

## 使用示例

### 基本使用

```rust
use http_cache_server::{
    HlsHandler,
    Loader,
    UrlMapper,
    BandwidthController,
    CacheStrategy,
    PerformanceOptimizer,
};
use std::sync::Arc;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<()> {
    // 初始化组件
    let loader = Arc::new(Loader::new());
    let url_mapper = Arc::new(UrlMapper::new());
    let bandwidth_controller = Arc::new(BandwidthController::new(1_000_000)); // 1Mbps
    
    // 创建 HLS 处理器
    let handler = HlsHandler::new(
        loader.clone(),
        url_mapper.clone(),
        bandwidth_controller.clone(),
    );

    // 配置缓存策略
    let storage = Arc::new(Storage::new("cache")?);
    let cache_strategy = Arc::new(CacheStrategy::new(
        storage.clone(),
        Duration::from_secs(3600), // 1小时过期
        1024 * 1024 * 1024,       // 1GB 容量限制
        5,                        // 最小访问次数
    ));

    // 配置性能优化
    let optimizer = Arc::new(PerformanceOptimizer::new(
        storage,
        1024 * 1024 * 5,         // 5MB 分片合并阈值
    ));

    // 启动缓存清理任务
    cache_strategy.clone().start_cleaning_task().await;

    // 处理 HLS 流
    handler.handle_adaptive_playback("http://example.com/stream.m3u8").await?;

    Ok(())
}
```

### 高级功能

```rust
// 智能预加载
handler.smart_preload("http://example.com/stream.m3u8", 5).await?;

// 缓存预热
handler.warmup_cache("http://example.com/master.m3u8", 3).await?;

// 直播流处理
handler.handle_live_stream_optimized(
    "http://example.com/live.m3u8",
    10,  // 最大分片数
    3,   // 合并阈值
).await?;
```

## 性能特性

- 智能预加载减少延迟
  - 基于访问模式的预测
  - 自适应预加载深度
  - 热点内容优先

- 高效的缓存管理
  - 多级清理策略
  - 动态容量调整
  - 访问频率感知

- 带宽优化
  - 自适应码率选择
  - 动态带宽控制
  - 令牌桶限流

- 资源利用
  - 智能分片合并
  - 并发请求控制
  - 内存使用优化

## 最佳实践

1. 合理配置
   - 根据系统资源设置合适的并发数
   - 设置适当的缓存容量限制
   - 调整带宽控制参数

2. 性能优化
   - 启用智能预加载
   - 配置合适的分片合并阈值
   - 利用缓存预热

3. 运维建议
   - 定期监控磁盘使用
   - 观察带宽使用情况
   - 及时清理过期缓存

## 许可证

MIT License