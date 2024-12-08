use crate::plugin::Plugin;
use crate::error::PluginError;
use async_trait::async_trait;
use std::collections::HashSet;
use tokio::sync::RwLock;

#[derive(Debug)]
pub struct SecurityPlugin {
    allowed_origins: RwLock<HashSet<String>>,
    rate_limit: u32,
}

impl SecurityPlugin {
    pub fn new(rate_limit: u32) -> Self {
        Self {
            allowed_origins: RwLock::new(HashSet::new()),
            rate_limit,
        }
    }

    pub async fn add_allowed_origin(&self, origin: String) {
        let mut origins = self.allowed_origins.write().await;
        origins.insert(origin);
    }

    pub async fn check_origin(&self, origin: &str) -> Result<bool, PluginError> {
        let origins = self.allowed_origins.read().await;
        Ok(origins.contains(origin))
    }

    pub async fn check_rate_limit(&self, _client_id: &str) -> Result<bool, PluginError> {
        // TODO: 实现速率限制检查
        Ok(true)
    }
}

#[async_trait]
impl Plugin for SecurityPlugin {
    fn name(&self) -> &str {
        "security"
    }

    fn version(&self) -> &str {
        "1.0.0"
    }

    async fn init(&self) -> Result<(), PluginError> {
        Ok(())
    }

    async fn cleanup(&self) -> Result<(), PluginError> {
        Ok(())
    }

    async fn health_check(&self) -> Result<bool, PluginError> {
        Ok(true)
    }
}

impl Clone for SecurityPlugin {
    fn clone(&self) -> Self {
        Self {
            allowed_origins: RwLock::new(HashSet::new()),
            rate_limit: self.rate_limit,
        }
    }
} 