use std::path::{Path, PathBuf};
use serde::{Serialize, Deserialize};
use chrono::{DateTime, Utc};
use std::collections::HashMap;

// 重新导出所有类型
pub use self::playlist::{PlaylistType, PlaylistInfo, Variant, Segment};
pub use self::path::PathBuilder;

mod playlist {
    use super::*;

    /// HLS 播放列表类型
    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    pub enum PlaylistType {
        /// 主播放列表(包含多个变体流)
        Master,
        /// 媒体播放列表(包含分片信息)
        Media,
    }

    /// 变体流信息
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Variant {
        /// 带宽(bps)
        pub bandwidth: u64,
        /// 分辨率
        pub resolution: Option<String>,
        /// 变体播放列表URL
        pub url: String,
        /// URL哈希值
        pub hash: String,
    }

    /// 分片信息
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Segment {
        /// 序号
        pub sequence: u32,
        /// 时长(秒)
        pub duration: f32,
        /// 分片URL
        pub url: String,
        /// 文件大小(字节)
        pub size: Option<u64>,
        /// 是否已缓存
        pub cached: bool,
    }

    /// 播放列表信息
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct PlaylistInfo {
        /// 原始URL
        pub url: String,
        /// URL哈希值
        pub hash: String,
        /// 播放列表类型
        pub playlist_type: PlaylistType,
        /// 最后修改时间
        pub last_modified: DateTime<Utc>,
        /// 目标时长(仅媒体播放列表)
        pub target_duration: Option<f32>,
        /// 媒体序列号(仅媒体播放列表)
        pub media_sequence: Option<u32>,
        /// 变体流列表(仅主播放列表)
        pub variants: Vec<Variant>,
        /// 分片列表(仅媒体播放列表)
        pub segments: Vec<Segment>,
    }
}

mod path {
    use std::path::{Path, PathBuf};

    /// 缓存路径生成器
    #[derive(Debug, Clone)]
    pub struct PathBuilder {
        base_path: PathBuf,
    }

    impl PathBuilder {
        pub fn new<P: Into<PathBuf>>(base_path: P) -> Self {
            Self {
                base_path: base_path.into(),
            }
        }

        pub fn base_path(&self) -> &Path {
            &self.base_path
        }

        /// 获取主播放列表路径
        pub fn master_playlist(&self, hash: &str) -> PathBuf {
            self.base_path
                .join(hash)
                .join("playlists")
                .join("master.m3u8")
        }

        /// 获取变体播放列表路径
        pub fn variant_playlist(&self, master_hash: &str, variant_name: &str) -> PathBuf {
            self.base_path
                .join(master_hash)
                .join("playlists")
                .join("variants")
                .join(format!("{}.m3u8", variant_name))
        }

        /// 获取媒体播放列表路径
        pub fn media_playlist(&self, hash: &str) -> PathBuf {
            self.base_path
                .join(hash)
                .join("playlists")
                .join("playlist.m3u8")
        }

        /// 获取分片路径
        pub fn segment(&self, playlist_hash: &str, variant: Option<&str>, sequence: u32) -> PathBuf {
            let mut path = self.base_path
                .join(playlist_hash)
                .join("segments");
                
            if let Some(variant) = variant {
                path = path.join(variant);
            }
            
            path.join(format!("{}.ts", sequence))
        }
    }
}

/// HLS 缓存状态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HLSState {
    /// 播放列表映射
    pub playlists: HashMap<String, PlaylistInfo>,
}

impl Default for HLSState {
    fn default() -> Self {
        Self {
            playlists: HashMap::new(),
        }
    }
} 