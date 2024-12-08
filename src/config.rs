use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub server: ServerConfig,
    pub plugins: PluginsConfig,
}

#[derive(Debug, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Deserialize)]
pub struct PluginsConfig {
    pub hls: HLSConfig,
    pub storage: StorageConfig,
    pub security: SecurityConfig,
}

#[derive(Debug, Deserialize)]
pub struct HLSConfig {
    pub cache_dir: PathBuf,
    pub segment_duration: u32,
    pub max_segments: usize,
}

#[derive(Debug, Deserialize)]
pub struct StorageConfig {
    pub root_path: PathBuf,
    pub max_size: u64,
}

#[derive(Debug, Deserialize)]
pub struct SecurityConfig {
    pub rate_limit: u32,
    pub allowed_origins: Vec<String>,
}

impl Config {
    pub fn load() -> Result<Self, Box<dyn std::error::Error>> {
        // 首先尝试从环境变量加载
        if let Ok(config_path) = std::env::var("CONFIG_PATH") {
            return Self::from_file(&config_path);
        }

        // 否则使用默认配置
        Ok(Self::default())
    }

    fn from_file(path: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let content = std::fs::read_to_string(path)?;
        Ok(toml::from_str(&content)?)
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server: ServerConfig {
                host: "127.0.0.1".to_string(),
                port: 8080,
            },
            plugins: PluginsConfig {
                hls: HLSConfig {
                    cache_dir: "./cache/hls".into(),
                    segment_duration: 10,
                    max_segments: 30,
                },
                storage: StorageConfig {
                    root_path: "./storage".into(),
                    max_size: 1024 * 1024 * 1024, // 1GB
                },
                security: SecurityConfig {
                    rate_limit: 100,
                    allowed_origins: vec![],
                },
            },
        }
    }
} 