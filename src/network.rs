use hyper::{Client, Body, Request};
use hyper::client::HttpConnector;
use crate::Result;

pub struct NetworkClient {
    client: Client<HttpConnector, Body>,
}

impl NetworkClient {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }

    pub async fn get(&self, url: &str) -> Result<Vec<u8>> {
        let req = Request::builder()
            .method("GET")
            .uri(url)
            .body(Body::empty())
            .map_err(|e| crate::error::CacheError::Network(e.to_string()))?;

        let resp = self.client.request(req).await
            .map_err(|e| crate::error::CacheError::Network(e.to_string()))?;

        let body_bytes = hyper::body::to_bytes(resp.into_body()).await
            .map_err(|e| crate::error::CacheError::Network(e.to_string()))?;

        Ok(body_bytes.to_vec())
    }
} 