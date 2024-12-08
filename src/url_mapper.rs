use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use url::Url;
use sha2::{Sha256, Digest};
use crate::error::PluginError;
use tracing::{info, warn, debug};

#[derive(Debug)]
pub struct UrlMapper {
    mappings: Arc<RwLock<HashMap<String, UrlMapping>>>,
    config: MapperConfig,
}

#[derive(Debug)]
struct UrlMapping {
    source_url: String,
    cache_key: String,
    content_type: String,
    last_modified: Option<String>,
    etag: Option<String>,
    created_at: chrono::DateTime<chrono::Utc>,
    access_count: u64,
}

#[derive(Debug)]
pub struct MapperConfig {
    pub strip_query: bool,
    pub normalize_path: bool,
    pub ignore_case: bool,
    pub max_mappings: usize,
}

impl Default for MapperConfig {
    fn default() -> Self {
        Self {
            strip_query: true,
            normalize_path: true,
            ignore_case: true,
            max_mappings: 10000,
        }
    }
}

impl UrlMapper {
    pub fn new(config: MapperConfig) -> Self {
        info!("Initializing URL mapper with config: {:?}", config);
        Self {
            mappings: Arc::new(RwLock::new(HashMap::new())),
            config,
        }
    }

    pub async fn add_mapping(
        &self,
        url: &str,
        content_type: &str,
        last_modified: Option<String>,
        etag: Option<String>,
    ) -> Result<String, PluginError> {
        debug!("Adding mapping for URL: {}", url);
        
        // 检查映射数量限制
        let mappings = self.mappings.read().await;
        if mappings.len() >= self.config.max_mappings {
            warn!("Maximum number of mappings reached: {}", self.config.max_mappings);
            return Err(PluginError::Storage("Maximum mappings limit reached".into()));
        }
        drop(mappings);

        // 解析和验证 URL
        let parsed_url = Url::parse(url)
            .map_err(|e| {
                warn!("Invalid URL {}: {}", url, e);
                PluginError::Network(format!("Invalid URL: {}", e))
            })?;

        // 生成缓存键
        let cache_key = self.generate_cache_key(&parsed_url)?;
        debug!("Generated cache key: {}", cache_key);

        let mut mappings = self.mappings.write().await;
        mappings.insert(url.to_string(), UrlMapping {
            source_url: url.to_string(),
            cache_key: cache_key.clone(),
            content_type: content_type.to_string(),
            last_modified,
            etag,
            created_at: chrono::Utc::now(),
            access_count: 0,
        });

        info!("Added mapping: {} -> {}", url, cache_key);
        Ok(cache_key)
    }

    pub async fn get_mapping(&self, url: &str) -> Option<MappingInfo> {
        debug!("Looking up mapping for URL: {}", url);
        let mut mappings = self.mappings.write().await;
        
        if let Some(mapping) = mappings.get_mut(url) {
            mapping.access_count += 1;
            debug!("Found mapping for URL: {} (accessed {} times)", mapping.source_url, mapping.access_count);
            
            Some(MappingInfo {
                cache_key: mapping.cache_key.clone(),
                content_type: mapping.content_type.clone(),
                last_modified: mapping.last_modified.clone(),
                etag: mapping.etag.clone(),
                created_at: mapping.created_at,
                access_count: mapping.access_count,
            })
        } else {
            debug!("No mapping found for URL: {}", url);
            None
        }
    }

    pub async fn update_mapping(
        &self,
        url: &str,
        last_modified: Option<String>,
        etag: Option<String>,
    ) -> Result<(), PluginError> {
        debug!("Updating mapping for URL: {}", url);
        let mut mappings = self.mappings.write().await;
        
        if let Some(mapping) = mappings.get_mut(url) {
            mapping.last_modified = last_modified;
            mapping.etag = etag;
            info!("Updated mapping for URL: {}", url);
            Ok(())
        } else {
            warn!("Mapping not found for URL: {}", url);
            Err(PluginError::Network("Mapping not found".into()))
        }
    }

    pub async fn remove_mapping(&self, url: &str) -> bool {
        debug!("Removing mapping for URL: {}", url);
        let mut mappings = self.mappings.write().await;
        
        if mappings.remove(url).is_some() {
            info!("Removed mapping for URL: {}", url);
            true
        } else {
            debug!("No mapping found to remove for URL: {}", url);
            false
        }
    }

    pub async fn get_stats(&self) -> MapperStats {
        let mappings = self.mappings.read().await;
        let now = chrono::Utc::now();
        
        let stats = MapperStats {
            total_mappings: mappings.len(),
            total_accesses: mappings.values().map(|m| m.access_count).sum(),
            with_etag: mappings.values().filter(|m| m.etag.is_some()).count(),
            with_last_modified: mappings.values().filter(|m| m.last_modified.is_some()).count(),
            avg_age: if !mappings.is_empty() {
                mappings.values()
                    .map(|m| (now - m.created_at).num_seconds() as f64)
                    .sum::<f64>() / mappings.len() as f64
            } else {
                0.0
            },
        };
        
        debug!("URL mapper stats: {:?}", stats);
        stats
    }

    fn generate_cache_key(&self, url: &Url) -> Result<String, PluginError> {
        let mut url = url.clone();

        // 规范化 URL
        if self.config.strip_query {
            url.set_query(None);
        }
        
        let mut path = url.path().to_string();
        if self.config.normalize_path {
            path = path.replace("//", "/");
            if path.ends_with('/') {
                path.pop();
            }
        }
        
        if self.config.ignore_case {
            path = path.to_lowercase();
        }

        // 生成哈希
        let mut hasher = Sha256::new();
        hasher.update(path.as_bytes());
        let result = hasher.finalize();
        
        Ok(format!("{:x}", result))
    }
}

#[derive(Debug, Clone)]
pub struct MappingInfo {
    pub cache_key: String,
    pub content_type: String,
    pub last_modified: Option<String>,
    pub etag: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub access_count: u64,
}

#[derive(Debug)]
pub struct MapperStats {
    pub total_mappings: usize,
    pub total_accesses: u64,
    pub with_etag: usize,
    pub with_last_modified: usize,
    pub avg_age: f64,
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_basic_mapping() {
        let mapper = UrlMapper::new(MapperConfig::default());
        let url = "https://example.com/test.jpg";
        
        let cache_key = mapper.add_mapping(
            url,
            "image/jpeg",
            Some("Wed, 21 Oct 2015 07:28:00 GMT".to_string()),
            Some("\"123456\"".to_string()),
        ).await.unwrap();
        
        let mapping = mapper.get_mapping(url).await.unwrap();
        assert_eq!(mapping.cache_key, cache_key);
        assert_eq!(mapping.content_type, "image/jpeg");
    }

    #[tokio::test]
    async fn test_mapping_stats() {
        let mapper = UrlMapper::new(MapperConfig::default());
        
        // Add some mappings
        mapper.add_mapping(
            "https://example.com/1.jpg",
            "image/jpeg",
            None,
            Some("\"123\"".to_string()),
        ).await.unwrap();
        
        mapper.add_mapping(
            "https://example.com/2.jpg",
            "image/jpeg",
            Some("Wed, 21 Oct 2015 07:28:00 GMT".to_string()),
            None,
        ).await.unwrap();
        
        // Get some mappings to increment access counts
        mapper.get_mapping("https://example.com/1.jpg").await;
        mapper.get_mapping("https://example.com/1.jpg").await;
        mapper.get_mapping("https://example.com/2.jpg").await;
        
        let stats = mapper.get_stats().await;
        assert_eq!(stats.total_mappings, 2);
        assert_eq!(stats.total_accesses, 3);
        assert_eq!(stats.with_etag, 1);
        assert_eq!(stats.with_last_modified, 1);
    }
} 