use std::path::PathBuf;
use std::time::{Duration, Instant};
use serde::{Serialize, Deserialize};
use super::metadata::CacheMetadata;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry {
    pub path: PathBuf,
    pub size: u64,
    #[serde(with = "instant_serde")]
    pub last_access: Instant,
    pub metadata: CacheMetadata,
}

// 自定义序列化 Instant
mod instant_serde {
    use serde::{Serializer, Deserialize, Deserializer};
    use std::time::{Duration, Instant};

    pub fn serialize<S>(instant: &Instant, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let duration = instant.elapsed();
        serializer.serialize_u64(duration.as_secs())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Instant, D::Error>
    where
        D: Deserializer<'de>,
    {
        let secs = <u64>::deserialize(deserializer)?;
        Ok(Instant::now() - Duration::from_secs(secs))
    }
}

impl CacheEntry {
    pub fn is_expired(&self, ttl: Duration) -> bool {
        Instant::now().duration_since(self.last_access) > ttl
    }

    pub fn touch(&mut self) {
        self.last_access = Instant::now();
    }
}