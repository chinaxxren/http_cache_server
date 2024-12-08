use async_trait::async_trait;
use crate::error::PluginError;
use std::fmt::Debug;

#[async_trait]
pub trait Plugin: Send + Sync + Debug {
    /// 获取插件名称
    fn name(&self) -> &str;
    
    /// 获取插件版本
    fn version(&self) -> &str {
        "0.1.0"
    }
    
    /// 插件初始化
    async fn init(&self) -> Result<(), PluginError> {
        Ok(())
    }
    
    /// 插件清理
    async fn cleanup(&self) -> Result<(), PluginError> {
        Ok(())
    }
    
    /// 插件状态检查
    async fn health_check(&self) -> Result<bool, PluginError> {
        Ok(true)
    }
} 