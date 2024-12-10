use async_trait::async_trait;
use crate::error::PluginError;

#[async_trait]
pub trait Plugin: Send + Sync {
    /// 获取插件名称
    fn name(&self) -> &str;

    /// 获取插件版本
    fn version(&self) -> &str;

    /// 初始化插件
    async fn init(&self) -> Result<(), PluginError>;

    /// 清理插件资源
    async fn cleanup(&self) -> Result<(), PluginError>;

    /// 健康检查
    async fn health_check(&self) -> Result<bool, PluginError> {
        Ok(true)  // 默认实现
    }
} 