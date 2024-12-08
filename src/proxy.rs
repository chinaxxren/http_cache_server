use std::sync::Arc;
use std::net::SocketAddr;

use hyper::{Body, Request, Response, Server};
use hyper::service::{make_service_fn, service_fn};
use hyper::server::conn::AddrStream;
use tracing::{info, warn, error, debug};

use crate::error::PluginError;
use crate::plugin::Plugin;
use crate::plugins::{
    hls::HLSPlugin,
    cache::CacheManager,
};

#[derive(Debug)]
pub struct ProxyServer {
    addr: SocketAddr,
    hls_plugin: Arc<HLSPlugin>,
    cache: Arc<CacheManager>,
}

impl ProxyServer {
    pub fn new(addr: SocketAddr, hls_plugin: Arc<HLSPlugin>, cache: Arc<CacheManager>) -> Self {
        info!("Creating new proxy server on {}", addr);
        Self {
            addr,
            hls_plugin,
            cache,
        }
    }

    #[tracing::instrument(skip(self))]
    pub async fn run(&self) -> Result<(), PluginError> {
        info!("Starting proxy server on {}", self.addr);

        let hls_plugin = self.hls_plugin.clone();
        let cache = self.cache.clone();
        
        let make_svc = make_service_fn(move |_conn: &AddrStream| {
            let hls_plugin = hls_plugin.clone();
            let cache = cache.clone();
            
            async move {
                Ok::<_, hyper::Error>(service_fn(move |req| {
                    let hls_plugin = hls_plugin.clone();
                    let cache = cache.clone();
                    Self::handle_request(req, hls_plugin, cache)
                }))
            }
        });

        let server = Server::bind(&self.addr).serve(make_svc);
        info!("Proxy server is ready to accept connections");

        if let Err(e) = server.await {
            error!("Server error: {}", e);
            return Err(PluginError::Network(e.to_string()));
        }

        Ok(())
    }

    #[tracing::instrument(skip(hls_plugin))]
    async fn handle_request(
        req: Request<Body>,
        hls_plugin: Arc<HLSPlugin>,
        cache: Arc<CacheManager>,
    ) -> Result<Response<Body>, hyper::Error> {
        let uri = req.uri().to_string();
        
        match hls_plugin.handle_hls_request(&uri).await {
            Ok(data) => {
                Ok(Response::new(Body::from(data)))
            }
            Err(e) => {
                warn!("Failed to serve request {}: {}", uri, e);
                Ok(Response::builder()
                    .status(500)
                    .body(Body::from(format!("Error: {}", e)))
                    .unwrap())
            }
        }
    }

    /// 获取代理 URL
    pub fn get_proxy_url(&self, original_url: &str) -> Result<String, PluginError> {
        let path = original_url.trim_start_matches("http://")
            .trim_start_matches("https://")
            .trim_start_matches("//");
            
        Ok(format!("http://{}/{}", self.addr, path))
    }
}

#[async_trait::async_trait]
impl Plugin for ProxyServer {
    fn name(&self) -> &str {
        "proxy"
    }

    fn version(&self) -> &str {
        "1.0.0"
    }

    #[tracing::instrument(skip(self))]
    async fn init(&self) -> Result<(), PluginError> {
        info!("Initializing proxy server plugin");
        Ok(())
    }

    #[tracing::instrument(skip(self))]
    async fn cleanup(&self) -> Result<(), PluginError> {
        info!("Cleaning up proxy server plugin");
        Ok(())
    }

    #[tracing::instrument(skip(self))]
    async fn health_check(&self) -> Result<bool, PluginError> {
        debug!("Performing proxy server health check");
        // TODO: 实现实际的健康检查逻辑
        Ok(true)
    }
}

impl Clone for ProxyServer {
    fn clone(&self) -> Self {
        Self {
            addr: self.addr,
            hls_plugin: self.hls_plugin.clone(),
            cache: self.cache.clone(),
        }
    }
} 