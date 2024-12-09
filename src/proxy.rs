use std::sync::Arc;
use std::net::SocketAddr;
use std::collections::HashMap;
use tokio::sync::RwLock;
use hyper::{Body, Request, Response, Server};
use hyper::service::{make_service_fn, service_fn};
use hyper::server::conn::AddrStream;
use tracing::{info, warn, error, debug};
use async_trait::async_trait;
use uuid::Uuid;

use crate::error::PluginError;
use crate::plugin::Plugin;
use crate::plugins::cache::CacheManager;

#[async_trait]
pub trait MediaHandler: Plugin + Send + Sync {
    async fn handle_request(&self, uri: &str) -> Result<Vec<u8>, PluginError>;
    async fn stream_request(&self, uri: &str, sender: hyper::body::Sender) -> Result<(), PluginError>;
    fn can_handle(&self, uri: &str) -> bool;
}

pub struct ProxyServer {
    addr: SocketAddr,
    handlers: Vec<Arc<dyn MediaHandler>>,
    cache: Arc<CacheManager>,
    url_mappings: Arc<RwLock<HashMap<String, String>>>, // 代理URL -> 真实URL
}

impl ProxyServer {
    pub fn new(addr: SocketAddr, cache: Arc<CacheManager>) -> Self {
        info!("Creating new proxy server on {}", addr);
        Self {
            addr,
            handlers: Vec::new(),
            cache,
            url_mappings: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn add_handler(&mut self, handler: Arc<dyn MediaHandler>) {
        info!("Adding new handler: {}", handler.name());
        self.handlers.push(handler);
    }

    pub async fn get_proxy_url(&self, original_url: &str) -> Result<String, PluginError> {
        // 生成唯一的代理路径
        let proxy_path = Uuid::new_v4().to_string();
        let proxy_url = format!("http://{}/{}", self.addr, proxy_path);
        
        // 保存映射关系
        let mut mappings = self.url_mappings.write().await;
        mappings.insert(proxy_path, original_url.to_string());
        
        info!("Created proxy mapping: {} -> {}", proxy_url, original_url);
        Ok(proxy_url)
    }

    #[tracing::instrument(skip(self))]
    pub async fn run(&self) -> Result<(), PluginError> {
        info!("Starting proxy server on {}", self.addr);
        debug!("Registered handlers: {}", self.handlers.len());

        let handlers = self.handlers.clone();
        let cache = self.cache.clone();
        let url_mappings = self.url_mappings.clone();
        
        let make_svc = make_service_fn(move |conn: &AddrStream| {
            let remote_addr = conn.remote_addr();
            info!("New connection from: {}", remote_addr);
            
            let handlers = handlers.clone();
            let cache = cache.clone();
            let url_mappings = url_mappings.clone();
            
            async move {
                Ok::<_, hyper::Error>(service_fn(move |req| {
                    let handlers = handlers.clone();
                    let cache = cache.clone();
                    let url_mappings = url_mappings.clone();
                    Self::handle_request(req, handlers, cache, url_mappings)
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
        url_mappings: Arc<RwLock<HashMap<String, String>>>,
    ) -> Result<Response<Body>, hyper::Error> {
        let proxy_path = req.uri().path().trim_start_matches('/');
        info!("Received proxy request: {}", proxy_path);

        // 查找真实 URL
        let real_url = {
            let mappings = url_mappings.read().await;
            mappings.get(proxy_path).cloned()
        };

        match real_url {
            Some(real_url) => {
                info!("Found real URL: {}", real_url);
                
                for handler in handlers.iter() {
                    if handler.can_handle(&real_url) {
                        info!("Handler {} will process request", handler.name());
                        
                        // 创建流式响应
                        let (sender, body) = Body::channel();
                        let handler = handler.clone();
                        let real_url = real_url.clone();

                        // 启动异步处理
                        tokio::spawn(async move {
                            match handler.stream_request(&real_url, sender).await {
                                Ok(_) => debug!("Stream completed successfully"),
                                Err(e) => error!("Stream error: {}", e),
                            }
                        });

                        return Ok(Response::new(body));
                    }
                }
                
                warn!("No handler found for URL: {}", real_url);
                Ok(Response::builder()
                    .status(404)
                    .body(Body::from("No handler found for this request"))
                    .unwrap())
            }
            None => {
                warn!("No mapping found for proxy path: {}", proxy_path);
                Ok(Response::builder()
                    .status(404)
                    .body(Body::from("Invalid proxy URL"))
                    .unwrap())
            }
        }
    }
}

impl Clone for ProxyServer {
    fn clone(&self) -> Self {
        Self {
            addr: self.addr,
            handlers: self.handlers.clone(),
            cache: self.cache.clone(),
            url_mappings: self.url_mappings.clone(),
        }
    }
} 