use crate::{Result, error::CacheError};
use hyper::{Body, Request, Response, Server};
use std::net::SocketAddr;
use std::convert::Infallible;
use std::sync::Arc;
use tokio::net::TcpListener;
use hyper::service::{make_service_fn, service_fn};
use crate::loader::Loader;
use crate::url_mapper::UrlMapper;

pub struct ProxyServer {
    addr: SocketAddr,
    loader: Arc<Loader>,
    url_mapper: Arc<UrlMapper>,
}

impl ProxyServer {
    pub fn new(addr: SocketAddr, loader: Arc<Loader>, url_mapper: Arc<UrlMapper>) -> Self {
        Self { 
            addr,
            loader,
            url_mapper,
        }
    }

    pub async fn run(&self) -> Result<()> {
        let loader = self.loader.clone();
        let url_mapper = self.url_mapper.clone();
        
        let listener = TcpListener::bind(self.addr).await
            .map_err(|e| CacheError::Io(e))?;
            
        let make_svc = make_service_fn(move |_| {
            let loader = loader.clone();
            let url_mapper = url_mapper.clone();
            
            async move {
                Ok::<_, Infallible>(service_fn(move |req| {
                    handle_request(req, loader.clone(), url_mapper.clone())
                }))
            }
        });

        let server = Server::builder(hyper::server::conn::AddrIncoming::from_listener(listener)?)
            .serve(make_svc);

        if let Err(e) = server.await {
            return Err(CacheError::Network(e.to_string()));
        }
        Ok(())
    }
}

async fn handle_request(
    req: Request<Body>, 
    loader: Arc<Loader>,
    url_mapper: Arc<UrlMapper>,
) -> std::result::Result<Response<Body>, hyper::Error> {
    match handle_request_internal(req, loader, url_mapper).await {
        Ok(response) => Ok(response),
        Err(e) => {
            let mut response = Response::new(Body::from(format!("Error: {}", e)));
            *response.status_mut() = hyper::StatusCode::INTERNAL_SERVER_ERROR;
            Ok(response)
        }
    }
}

async fn handle_request_internal(
    req: Request<Body>,
    loader: Arc<Loader>,
    url_mapper: Arc<UrlMapper>,
) -> Result<Response<Body>> {
    // 获取请求URL
    let uri = req.uri().to_string();
    
    // 映射URL
    let mapped_url = url_mapper.map_url(&uri)?;
    
    // 加载资源
    let data = loader.load(&mapped_url).await?;
    
    // 构建响应
    let response = Response::builder()
        .status(200)
        .header("content-type", "application/octet-stream")
        .header("content-length", data.len().to_string())
        .body(Body::from(data))
        .map_err(|e| CacheError::Network(e.to_string()))?;
    
    Ok(response)
} 