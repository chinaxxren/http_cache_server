use hyper::{Request, Body, Uri};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

/// 从请求中获取完整的 URL 字符串
pub fn get_request_url(req: &Request<Body>) -> String {
    req.uri().to_string()
}

/// 从 Uri 获取完整的 URL 字符串
pub fn get_full_url(uri: &Uri) -> String {
    if uri.scheme().is_none() {
        format!("http://{}{}", 
            uri.authority().map(|a| a.as_str()).unwrap_or(""),
            uri.path_and_query().map(|p| p.as_str()).unwrap_or("")
        )
    } else {
        uri.to_string()
    }
}

/// 从 URL 字符串中提取路径部分
pub fn get_url_path(url: &str) -> &str {
    url.split('?')
        .next()
        .unwrap_or(url)
        .split('#')
        .next()
        .unwrap_or(url)
}

/// 计算URL的哈希值
/// 
/// # Arguments
/// * `url` - 要计算哈希的URL字符串
/// 
/// # Returns
/// 返回16位的十六进制哈希字符串
/// 
/// # Examples
/// ```
/// use http_cache_server::utils::hash_url;
/// 
/// let url = "http://example.com/video.m3u8";
/// let hash = hash_url(url);
/// assert_eq!(hash.len(), 16);
/// ```
pub fn hash_url(url: &str) -> String {
    let mut hasher = DefaultHasher::new();
    url.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// 检查URL是否为绝对URL
/// 
/// # Arguments
/// * `url` - 要检查的URL字符串
/// 
/// # Returns
/// 如果是绝对URL返回true，否则返回false
pub fn is_absolute_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

/// 将相对URL转换为绝对URL
/// 
/// # Arguments
/// * `base` - 基础URL
/// * `relative` - 相对URL
/// 
/// # Returns
/// 返回绝对URL字符串
/// 
/// # Examples
/// ```
/// use http_cache_server::utils::resolve_url;
/// 
/// let base = "http://example.com/video/";
/// let relative = "segment.ts";
/// let absolute = resolve_url(base, relative);
/// assert_eq!(absolute, "http://example.com/video/segment.ts");
/// ```
pub fn resolve_url(base: &str, relative: &str) -> String {
    if is_absolute_url(relative) {
        relative.to_string()
    } else {
        let base = url::Url::parse(base).unwrap();
        base.join(relative).unwrap().to_string()
    }
}

/// 获取存储路径结构
pub fn get_storage_paths(url: &str) -> (PathBuf, PathBuf) {
    if let Ok(parsed_url) = url::Url::parse(url) {
        let path_segments: Vec<&str> = parsed_url.path()
            .split('/')
            .filter(|s| !s.is_empty())
            .collect();

        // 获取请求的基础 URL 哈希
        let base_url = format!("{}://{}", parsed_url.scheme(), parsed_url.host_str().unwrap_or(""));
        let request_hash = hash_url(&base_url);
        let base_path = PathBuf::from("cache/hls").join(&request_hash);

        // 构建文件路径
        let file_path = if let Some(last_segment) = path_segments.last() {
            // 找到 bipbop 目录的位置
            if let Some(bipbop_pos) = path_segments.iter().position(|&s| s.contains("bipbop")) {
                // 获取 bipbop 后的所有路径段
                let sub_segments = &path_segments[bipbop_pos + 1..];
                
                match *last_segment {
                    "bipbopall.m3u8" => {
                        // 主播放列表
                        base_path.join("bipbopall.m3u8")
                    },
                    "prog_index.m3u8" | "playlist.m3u8" => {
                        // 子播放列表
                        if let Some(gear) = sub_segments.iter().find(|&s| s.starts_with("gear")) {
                            base_path.join(gear).join("prog_index.m3u8")
                        } else {
                            base_path.join("playlist.m3u8")
                        }
                    },
                    _ => {
                        if let Some(gear) = sub_segments.iter().find(|&s| s.starts_with("gear")) {
                            // gear 目录下的文件
                            if last_segment.starts_with("fileSequence") {
                                // 分片文件
                                base_path.join(gear).join("segments").join(last_segment)
                            } else {
                                base_path.join(gear).join(last_segment)
                            }
                        } else {
                            base_path.join(last_segment)
                        }
                    }
                }
            } else {
                // 非 bipbop 路径
                if last_segment.ends_with(".m3u8") {
                    base_path.join("playlist.m3u8")
                } else if last_segment.ends_with(".ts") {
                    base_path.join("segments").join(last_segment)
                } else {
                    base_path.join(last_segment)
                }
            }
        } else {
            base_path.join("unknown")
        };

        (base_path, file_path)
    } else {
        // 如果 URL 解析失败，使用简单的哈希路径
        let hash = hash_url(url);
        let base_path = PathBuf::from("cache/hls").join(&hash);
        let file_path = base_path.join("unknown");
        (base_path, file_path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hyper::Uri;

    #[test]
    fn test_get_full_url() {
        let uri: Uri = "http://example.com/path?query=1".parse().unwrap();
        assert_eq!(get_full_url(&uri), "http://example.com/path?query=1");

        let uri: Uri = "/path?query=1".parse().unwrap();
        assert_eq!(get_full_url(&uri), "http:///path?query=1");
    }

    #[test]
    fn test_get_url_path() {
        assert_eq!(get_url_path("http://example.com/path?query=1"), "http://example.com/path");
        assert_eq!(get_url_path("/path?query=1#fragment"), "/path");
        assert_eq!(get_url_path("/path"), "/path");
    }

    #[test]
    fn test_hash_url() {
        let url = "http://example.com/video.m3u8";
        let hash = hash_url(url);
        println!("hash: {}", hash);
        assert_eq!(hash.len(), 16);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_is_absolute_url() {
        assert!(is_absolute_url("http://example.com"));
        assert!(is_absolute_url("https://example.com"));
        assert!(!is_absolute_url("relative/path"));
        assert!(!is_absolute_url("/absolute/path"));
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

        let base = "http://example.com/video/playlist.m3u8";
        let relative = "../segments/segment.ts";
        assert_eq!(
            resolve_url(base, relative),
            "http://example.com/segments/segment.ts"
        );
    }

    #[test]
    fn test_get_storage_paths() {
        // 测试主播放列表
        let url = "http://devimages.apple.com/iphone/samples/bipbop/bipbopall.m3u8";
        let (_, file_path) = get_storage_paths(url);
        assert!(file_path.ends_with("bipbopall.m3u8"));

        // 测试子播放列表
        let url = "http://devimages.apple.com/iphone/samples/bipbop/gear1/prog_index.m3u8";
        let (_, file_path) = get_storage_paths(url);
        assert!(file_path.ends_with("gear1/prog_index.m3u8"));

        // 测试分片文件
        let url = "http://devimages.apple.com/iphone/samples/bipbop/gear1/fileSequence0.ts";
        let (_, file_path) = get_storage_paths(url);
        assert!(file_path.ends_with("gear1/segments/fileSequence0.ts"));

        // 测试非 bipbop 路径
        let test_cases = [
            ("http://example.com/video/playlist.m3u8", "playlist.m3u8"),
            ("http://example.com/video/segment1.ts", "segments/segment1.ts"),
            ("http://example.com/video/other.txt", "other.txt"),
        ];

        for (input, expected) in test_cases.iter() {
            let (_, file_path) = get_storage_paths(input);
            assert!(file_path.ends_with(expected), "Failed for {}", input);
        }

        // 测试 bipbop 路径结构
        let test_cases = [
            ("bipbopall.m3u8", "bipbopall.m3u8"),
            ("gear1/prog_index.m3u8", "gear1/prog_index.m3u8"),
            ("gear1/fileSequence0.ts", "gear1/segments/fileSequence0.ts"),
            ("gear2/prog_index.m3u8", "gear2/prog_index.m3u8"),
            ("gear2/fileSequence0.ts", "gear2/segments/fileSequence0.ts"),
            ("gear3/prog_index.m3u8", "gear3/prog_index.m3u8"),
            ("gear3/fileSequence0.ts", "gear3/segments/fileSequence0.ts"),
            ("gear4/prog_index.m3u8", "gear4/prog_index.m3u8"),
            ("gear4/fileSequence0.ts", "gear4/segments/fileSequence0.ts"),
            ("gear1/playlist.m3u8", "gear1/prog_index.m3u8"),
            ("gear2/playlist.m3u8", "gear2/prog_index.m3u8"),
            ("gear3/playlist.m3u8", "gear3/prog_index.m3u8"),
            ("gear4/playlist.m3u8", "gear4/prog_index.m3u8"),
        ];

        for (input, expected) in test_cases.iter() {
            let url = format!("http://devimages.apple.com/iphone/samples/bipbop/{}", input);
            let (_, file_path) = get_storage_paths(&url);
            assert!(file_path.ends_with(expected), "Failed for {}", input);
        }

        // 测试 URL 解析失败的情况
        let url = "invalid://url";
        let (_, file_path) = get_storage_paths(url);
        assert!(file_path.ends_with("unknown"));
    }
} 