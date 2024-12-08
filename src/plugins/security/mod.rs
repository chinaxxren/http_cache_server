use crate::plugin::Plugin;
use crate::error::PluginError;
use async_trait::async_trait;
use std::collections::HashSet;
use tokio::sync::RwLock;
use tracing::{info, warn, debug};

#[derive(Debug)]
pub struct SecurityPlugin {
    allowed_origins: RwLock<HashSet<String>>,
    rate_limit: u32,
}

impl SecurityPlugin {
    pub fn new(rate_limit: u32) -> Self {
        info!("Initializing SecurityPlugin with rate limit: {}", rate_limit);
        Self {
            allowed_origins: RwLock::new(HashSet::new()),
            rate_limit,
        }
    }

    pub async fn add_allowed_origin(&self, origin: String) {
        debug!("Adding allowed origin: {}", origin);
        let mut origins = self.allowed_origins.write().await;
        origins.insert(origin.clone());
        info!("Added new allowed origin: {}", origin);
    }

    pub async fn remove_allowed_origin(&self, origin: &str) {
        debug!("Removing allowed origin: {}", origin);
        let mut origins = self.allowed_origins.write().await;
        if origins.remove(origin) {
            info!("Removed allowed origin: {}", origin);
        } else {
            warn!("Attempted to remove non-existent origin: {}", origin);
        }
    }

    pub async fn check_origin(&self, origin: &str) -> Result<bool, PluginError> {
        debug!("Checking origin: {}", origin);
        let origins = self.allowed_origins.read().await;
        let allowed = origins.contains(origin);
        if allowed {
            debug!("Origin allowed: {}", origin);
        } else {
            warn!("Origin blocked: {}", origin);
        }
        Ok(allowed)
    }

    pub async fn check_rate_limit(&self, client_id: &str) -> Result<bool, PluginError> {
        debug!("Checking rate limit for client: {}", client_id);
        // TODO: 实现实际的速率限制检查
        Ok(true)
    }

    pub async fn get_allowed_origins(&self) -> HashSet<String> {
        let origins = self.allowed_origins.read().await;
        let origins_set = origins.clone();
        debug!("Current allowed origins: {:?}", origins_set);
        origins_set
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
        info!("Initializing security plugin");
        Ok(())
    }

    async fn cleanup(&self) -> Result<(), PluginError> {
        info!("Cleaning up security plugin");
        let origins = self.allowed_origins.read().await;
        info!("Clearing {} allowed origins", origins.len());
        Ok(())
    }

    async fn health_check(&self) -> Result<bool, PluginError> {
        let origins = self.allowed_origins.read().await;
        let status = !origins.is_empty();
        if status {
            info!("Security plugin healthy with {} allowed origins", origins.len());
        } else {
            warn!("Security plugin has no allowed origins configured");
        }
        Ok(status)
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