use crate::plugin::Plugin;
use crate::error::PluginError;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, error};

#[derive(Debug)]
pub struct PluginManager {
    plugins: RwLock<HashMap<String, Arc<dyn Plugin>>>,
}

impl PluginManager {
    pub fn new() -> Self {
        Self {
            plugins: RwLock::new(HashMap::new()),
        }
    }

    pub async fn register_plugin(&self, plugin: Arc<dyn Plugin>) -> Result<(), PluginError> {
        let plugin_name = plugin.name().to_string();
        info!("Registering plugin: {} v{}", plugin_name, plugin.version());
        
        let mut plugins = self.plugins.write().await;
        if plugins.contains_key(&plugin_name) {
            return Err(PluginError::Plugin(format!("Plugin {} already registered", plugin_name)));
        }

        match plugin.init().await {
            Ok(_) => {
                plugins.insert(plugin_name.clone(), plugin);
                info!("Successfully registered plugin: {}", plugin_name);
                Ok(())
            }
            Err(e) => {
                error!("Failed to initialize plugin {}: {}", plugin_name, e);
                Err(e)
            }
        }
    }

    pub async fn get_plugin(&self, name: &str) -> Option<Arc<dyn Plugin>> {
        self.plugins.read().await.get(name).cloned()
    }

    pub async fn cleanup(&self) -> Result<(), PluginError> {
        let plugins = self.plugins.read().await;
        for (name, plugin) in plugins.iter() {
            if let Err(e) = plugin.cleanup().await {
                error!("Error cleaning up plugin {}: {}", name, e);
            }
        }
        Ok(())
    }

    pub async fn health_check(&self) -> HashMap<String, bool> {
        let mut results = HashMap::new();
        let plugins = self.plugins.read().await;
        
        for (name, plugin) in plugins.iter() {
            match plugin.health_check().await {
                Ok(status) => results.insert(name.clone(), status),
                Err(_) => results.insert(name.clone(), false),
            };
        }
        results
    }
} 