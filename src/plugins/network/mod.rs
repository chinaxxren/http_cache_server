use std::sync::Arc;
use tokio::sync::RwLock;
use hyper::{Client, client::HttpConnector, Uri};
use crate::error::PluginError;
use std::time::Duration;
mod retry;
use retry::with_retry;

#[derive(Debug)]
pub struct NetworkManager {
    client: Client<HttpConnector>,
    settings: Arc<RwLock<NetworkSettings>>,
}

#[derive(Debug)]
struct NetworkSettings {
    _timeout: std::time::Duration,
    max_retries: u32,
}

impl NetworkManager {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
            settings: Arc::new(RwLock::new(NetworkSettings {
                _timeout: std::time::Duration::from_secs(30),
                max_retries: 3,
            })),
        }
    }

    pub async fn get(&self, url: &str) -> Result<bytes::Bytes, PluginError> {
        let settings = self.settings.read().await;
        let uri: Uri = url.parse()
            .map_err(|e: hyper::http::uri::InvalidUri| PluginError::Network(e.to_string()))?;
        
        with_retry(
            || async {
                let resp = self.client
                    .get(uri.clone())
                    .await
                    .map_err(|e: hyper::Error| PluginError::Network(e.to_string()))?;
                
                if !resp.status().is_success() {
                    return Err(PluginError::Network(format!("HTTP error: {}", resp.status())));
                }

                hyper::body::to_bytes(resp.into_body())
                    .await
                    .map_err(|e: hyper::Error| PluginError::Network(e.to_string()))
            },
            settings.max_retries,
            Duration::from_secs(1),
        ).await
    }

    #[allow(dead_code)]
    pub async fn set_timeout(&self, timeout: std::time::Duration) {
        let mut settings = self.settings.write().await;
        settings._timeout = timeout;
    }

    #[allow(dead_code)]
    pub async fn set_max_retries(&self, retries: u32) {
        let mut settings = self.settings.write().await;
        settings.max_retries = retries;
    }
}

impl Clone for NetworkManager {
    fn clone(&self) -> Self {
        Self {
            client: Client::new(),
            settings: self.settings.clone(),
        }
    }
} 