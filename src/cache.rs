use std::sync::Arc;
use dashmap::DashMap;
use crate::Result;

pub struct Cache {
    storage: Arc<DashMap<String, Vec<u8>>>,
}

impl Cache {
    pub fn new() -> Self {
        Self {
            storage: Arc::new(DashMap::new()),
        }
    }

    pub async fn get(&self, key: &str) -> Option<Vec<u8>> {
        self.storage.get(key).map(|v| v.clone())
    }

    pub async fn set(&self, key: String, value: Vec<u8>) -> Result<()> {
        self.storage.insert(key, value);
        Ok(())
    }
} 