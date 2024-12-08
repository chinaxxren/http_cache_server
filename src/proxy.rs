use crate::error::PluginError;
use crate::plugin::Plugin;
use crate::plugins::hls::HLSPlugin;
use hyper::{Body, Request, Response, Server};
use hyper::service::{make_service_fn, service_fn};
use hyper::server::conn::AddrStream;
use std::sync::Arc;
use std::net::SocketAddr;
use tracing::{info, warn, error, debug};

#[derive(Debug)]
pub struct ProxyServer {
    addr: SocketAddr,
    hls_plugin: Arc<HLSPlugin>,
}

impl ProxyServer {
    pub fn new(addr: SocketAddr, hls_plugin: Arc<HLSPlugin>) -> Self {
        info!("Creating new proxy server on {}", addr);
        Self {
            addr,
            hls_plugin,
        }
    }

    #[tracing::instrument(skip(self))]
    pub async fn run(&self) -> Result<(), PluginError> {
        info!("Starting proxy server on {}", self.addr);

        let hls_plugin = self.hls_plugin.clone();
        
        let make_svc = make_service_fn(|conn: &AddrStream| {
            let remote_addr = conn.remote_addr();
            debug!("New connection from {}", remote_addr);
            let hls_plugin = hls_plugin.clone();
            
            async move {
                Ok::<_, hyper::Error>(service_fn(move |req| {
                    let hls_plugin = hls_plugin.clone();
                    Self::handle_request(req, hls_plugin)
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
    ) -> Result<Response<Body>, hyper::Error> {
        let method = req.method().clone();
        let uri = req.uri().to_string();
        let stream_id = Self::extract_stream_id(&req)
            .unwrap_or_else(|| "default".to_string());

        debug!("Received {} request for {} (stream: {})", method, uri, stream_id);

        match *req.method() {
            hyper::Method::GET => {
                debug!("Processing GET request for stream {}", stream_id);

                match hls_plugin.download_segment(&uri, &stream_id).await {
                    Ok(data) => {
                        info!(
                            "Successfully served {} bytes for {} (stream: {})",
                            data.len(), uri, stream_id
                        );
                        Ok(Response::new(Body::from(data)))
                    }
                    Err(e) => {
                        warn!("Failed to serve request {} (stream: {}): {}", uri, stream_id, e);
                        Ok(Response::builder()
                            .status(500)
                            .body(Body::from(format!("Error: {}", e)))
                            .unwrap())
                    }
                }
            }
            _ => {
                warn!("Unsupported method: {} for {}", method, uri);
                Ok(Response::builder()
                    .status(405)
                    .body(Body::from("Method not allowed"))
                    .unwrap())
            }
        }
    }

    fn extract_stream_id(req: &Request<Body>) -> Option<String> {
        let stream_id = req.headers()
            .get("X-Stream-ID")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        
        if let Some(ref id) = stream_id {
            debug!("Extracted stream ID: {}", id);
        } else {
            debug!("No stream ID found in request");
        }
        
        stream_id
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
        }
    }
} 