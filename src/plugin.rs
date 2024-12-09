use async_trait::async_trait;
use crate::error::PluginError;

#[async_trait]
pub trait Plugin: Send + Sync {
    fn name(&self) -> &str;
    fn version(&self) -> &str;
    async fn init(&self) -> Result<(), PluginError>;
    async fn cleanup(&self) -> Result<(), PluginError>;
} 