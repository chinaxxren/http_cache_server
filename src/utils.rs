use hyper::{Request, Body, Uri};

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
} 