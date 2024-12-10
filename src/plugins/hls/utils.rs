use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// 计算URL的哈希值
pub fn hash_url(url: &str) -> String {
    let mut hasher = DefaultHasher::new();
    url.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// 将相对URL转换为绝对URL
pub fn resolve_url(base_url: &str, relative_url: &str) -> String {
    if relative_url.starts_with("http://") || relative_url.starts_with("https://") {
        relative_url.to_string()
    } else {
        let base = url::Url::parse(base_url).unwrap();
        base.join(relative_url).unwrap().to_string()
    }
}

/// 从URL中提取序列号
pub fn extract_sequence_number(uri: &str) -> Option<u32> {
    let parts: Vec<&str> = uri.split('/').collect();
    let last_part = parts.last()?;
    
    let file_parts: Vec<&str> = last_part.split('.').collect();
    let name_part = file_parts.first()?;
    
    name_part.replace("segment", "").parse().ok()
}

pub fn get_playlist_url(segment_url: &str) -> String {
    segment_url.split("/").take(segment_url.split("/").count() - 1).collect::<Vec<_>>().join("/")
}

pub fn get_segment_name(segment_url: &str) -> String {
    segment_url.split("/").last().unwrap_or("").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_sequence_number() {
        assert_eq!(extract_sequence_number("/path/segment123.ts"), Some(123));
        assert_eq!(extract_sequence_number("/path/123.ts"), Some(123));
        assert_eq!(extract_sequence_number("invalid"), None);
    }

    #[test]
    fn test_hash_url() {
        let url = "http://example.com/video.m3u8";
        let hash = hash_url(url);
        assert_eq!(hash.len(), 16);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_resolve_url() {
        let base = "http://example.com/video/";
        let relative = "segment.ts";
        assert_eq!(
            resolve_url(base, relative),
            "http://example.com/video/segment.ts"
        );

        let absolute = "http://other.com/file.ts";
        assert_eq!(resolve_url(base, absolute), absolute);
    }
} 