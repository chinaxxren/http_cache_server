use std::sync::Arc;
use std::net::SocketAddr;

use hyper::{Body, Request, Response, Server};
use hyper::service::{make_service_fn, service_fn};
use hyper::server::conn::AddrStream;
use tracing::{info, warn, error, debug};
use async_trait::async_trait;

use crate::error::PluginError;
use crate::plugin::Plugin;
use crate::plugins::cache::CacheManager;

#[async_trait]
pub trait MediaHandler: Plugin + Send + Sync {
    async fn handle_request(&self, uri: &str) -> Result<Vec<u8>, PluginError>;
    fn can_handle(&self, uri: &str) -> bool;
}

pub struct ProxyServer {
    addr: SocketAddr,
    handlers: Vec<Arc<dyn MediaHandler>>,
    cache: Arc<CacheManager>,
}

impl ProxyServer {
    pub fn new(addr: SocketAddr, cache: Arc<CacheManager>) -> Self {
        info!("Creating new proxy server on {}", addr);
        Self {
            addr,
            handlers: Vec::new(),
            cache,
        }
    }

    pub fn add_handler(&mut self, handler: Arc<dyn MediaHandler>) {
        info!("Adding new handler: {}", handler.name());
        self.handlers.push(handler);
    }

    #[tracing::instrument(skip(self))]
    pub async fn run(&self) -> Result<(), PluginError> {
        info!("Starting proxy server on {}", self.addr);
        debug!("Registered handlers: {}", self.handlers.len());

        let handlers = self.handlers.clone();
        let cache = self.cache.clone();
        
        let make_svc = make_service_fn(move |conn: &AddrStream| {
            let remote_addr = conn.remote_addr();
            info!("New connection from: {}", remote_addr);
            
            let handlers = handlers.clone();
            let cache = cache.clone();
            
            async move {
                Ok::<_, hyper::Error>(service_fn(move |req| {
                    debug!("Received request from {}: {} {}", 
                        remote_addr, req.method(), req.uri());
                    let handlers = handlers.clone();
                    let cache = cache.clone();
                    Self::handle_request(req, handlers, cache)
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

    #[tracing::instrument(skip(handlers))]
    async fn handle_request(
        req: Request<Body>,
        handlers: Vec<Arc<dyn MediaHandler>>,
        cache: Arc<CacheManager>,
    ) -> Result<Response<Body>, hyper::Error> {
        let uri = req.uri().to_string();
        info!("Processing request: {}", uri);
        
        for handler in handlers.iter() {
            if handler.can_handle(&uri) {
                info!("Handler {} will process request", handler.name());
                match handler.handle_request(&uri).await {
                    Ok(data) => {
                        info!("Request processed successfully, response size: {} bytes", data.len());
                        return Ok(Response::new(Body::from(data)));
                    }
                    Err(e) => {
                        warn!("Handler {} failed: {}", handler.name(), e);
                        continue;
                    }
                }
            }
        }

        warn!("No handler found for request: {}", uri);
        Ok(Response::builder()
            .status(404)
            .body(Body::from("No handler found for this request"))
            .unwrap())
    }

    pub fn get_proxy_url(&self, original_url: &str) -> Result<String, PluginError> {
        let path = original_url.trim_start_matches("http://")
            .trim_start_matches("https://")
            .trim_start_matches("//");
            
        Ok(format!("http://{}/{}", self.addr, path))
    }
}

impl Clone for ProxyServer {
    fn clone(&self) -> Self {
        Self {
            addr: self.addr,
            handlers: self.handlers.clone(),
            cache: self.cache.clone(),
        }
    }
} 