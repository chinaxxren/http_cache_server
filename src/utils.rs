use hyper::{Request, Body, Uri};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

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
} 