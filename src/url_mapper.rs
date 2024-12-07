use std::collections::HashMap;
use url::Url;
use crate::Result;
use crate::error::CacheError;

pub struct UrlMapper {
    rules: HashMap<String, String>,
}

impl UrlMapper {
    pub fn new() -> Self {
        Self {
            rules: HashMap::new()
        }
    }

    pub fn add_rule(&mut self, pattern: String, target: String) {
        self.rules.insert(pattern, target);
    }

    pub fn map_url(&self, url: &str) -> Result<String> {
        let parsed = Url::parse(url)
            .map_err(|e| CacheError::InvalidInput(e.to_string()))?;
        
        for (pattern, target) in &self.rules {
            if parsed.as_str().starts_with(pattern) {
                return Ok(parsed.as_str().replace(pattern, target));
            }
        }
        
        Ok(url.to_string())
    }

    pub fn generate_cache_key(&self, url: &str) -> Result<String> {
        let parsed = Url::parse(url)
            .map_err(|e| CacheError::InvalidInput(e.to_string()))?;
        
        // Remove query parameters that don't affect content
        let mut clean_url = parsed.clone();
        clean_url.set_query(None);
        
        Ok(clean_url.to_string())
    }
} 