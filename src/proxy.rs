use crate::config::Config;
use crate::health_check::HealthChecker;
use crate::load_balancer::Upstream;
use crate::router::DomainRouter;
use anyhow::Result;
use http_body_util::{BodyExt, Full};
use hyper::body::{Bytes, Incoming};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tracing::{debug, error, info, warn};

pub struct ReverseProxy {
    config: Config,
    router: Arc<DomainRouter>,
}

impl ReverseProxy {
    pub async fn new(config: Config) -> Result<Self> {
        let router = Arc::new(DomainRouter::new(&config));

        Ok(Self { config, router })
    }

    pub async fn start(self) -> Result<()> {
        let addr = SocketAddr::new(
            self.config.proxy.bind_address.parse()?,
            self.config.proxy.port,
        );

        info!("Starting reverse proxy on {}", addr);

        if let Some(health_check_config) = &self.config.load_balancer.health_check {
            let health_checker =
                HealthChecker::new(health_check_config.clone(), self.router.clone());
            tokio::spawn(async move {
                health_checker.start().await;
            });
        }

        let proxy = Arc::new(self);

        let listener = TcpListener::bind(addr).await?;
        info!("Listening on http://{}", addr);

        loop {
            let (stream, _) = listener.accept().await?;
            let io = TokioIo::new(stream);
            let proxy = proxy.clone();

            tokio::task::spawn(async move {
                if let Err(err) = http1::Builder::new()
                    .serve_connection(
                        io,
                        service_fn(move |req| {
                            let proxy = proxy.clone();
                            async move { proxy.handle_request(req).await }
                        }),
                    )
                    .await
                {
                    error!("Error serving connection: {:?}", err);
                }
            });
        }
    }

    async fn handle_request(
        &self,
        req: Request<Incoming>,
    ) -> Result<Response<Full<Bytes>>, Infallible> {
        debug!("Received {} request to {}", req.method(), req.uri().path());

        let host_header = req
            .headers()
            .get("host")
            .and_then(|h| h.to_str().ok())
            .map(|h| h.to_string());

        let (_load_balancer, upstream) =
            match self.router.route_request(host_header.as_deref()).await {
                Some((lb, upstream_opt)) => match upstream_opt {
                    Some(upstream) => (lb, upstream),
                    None => {
                        warn!(
                            "No healthy upstreams available for domain: {:?}",
                            host_header
                        );
                        return Ok(Response::builder()
                            .status(StatusCode::SERVICE_UNAVAILABLE)
                            .body(Full::new(Bytes::from("Service Unavailable")))
                            .unwrap());
                    }
                },
                None => {
                    warn!("Failed to route request for domain: {:?}", host_header);
                    return Ok(Response::builder()
                        .status(StatusCode::SERVICE_UNAVAILABLE)
                        .body(Full::new(Bytes::from("Service Unavailable")))
                        .unwrap());
                }
            };

        debug!(
            "Routing request for domain {:?} to upstream {}",
            host_header, upstream.id
        );

        let upstream_id = upstream.id.clone();
        self.router
            .increment_connections(&upstream_id, host_header.as_deref())
            .await;

        let result = self.proxy_request(req, &upstream).await;

        self.router
            .decrement_connections(&upstream_id, host_header.as_deref())
            .await;

        result
    }

    async fn proxy_request(
        &self,
        mut req: Request<Incoming>,
        upstream: &Upstream,
    ) -> Result<Response<Full<Bytes>>, Infallible> {
        let path_and_query = req
            .uri()
            .path_and_query()
            .map(|x| x.as_str())
            .unwrap_or("/");
        let upstream_uri = format!(
            "http://{}:{}{}",
            upstream.host, upstream.port, path_and_query
        );

        debug!("Proxying request to {}", upstream_uri);

        let uri = match upstream_uri.parse() {
            Ok(uri) => uri,
            Err(e) => {
                error!("Failed to parse upstream URI: {}", e);
                return Ok(Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .body(Full::new(Bytes::from("Bad Request")))
                    .unwrap());
            }
        };

        let method = req.method().clone();
        let headers_map = req.headers().clone();

        *req.uri_mut() = uri;

        let headers = req.headers_mut();
        headers.remove("connection");
        headers.remove("proxy-connection");
        headers.remove("te");
        headers.remove("trailers");
        headers.remove("upgrade");

        let body_bytes = match req.into_body().collect().await {
            Ok(collected) => collected.to_bytes(),
            Err(e) => {
                error!("Failed to read request body: {}", e);
                return Ok(Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .body(Full::new(Bytes::from("Bad Request")))
                    .unwrap());
            }
        };

        let client = reqwest::Client::new();

        let mut request_builder = client.request(method, &upstream_uri);

        for (name, value) in headers_map.iter() {
            if let Ok(header_name) =
                reqwest::header::HeaderName::from_bytes(name.as_str().as_bytes())
            {
                if let Ok(header_value) = reqwest::header::HeaderValue::from_bytes(value.as_bytes())
                {
                    request_builder = request_builder.header(header_name, header_value);
                }
            }
        }

        if !body_bytes.is_empty() {
            request_builder = request_builder.body(body_bytes.to_vec());
        }

        match request_builder.send().await {
            Ok(response) => {
                let status = response.status();
                let headers = response.headers().clone();
                let body_bytes = match response.bytes().await {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        error!("Failed to read response body: {}", e);
                        return Ok(Response::builder()
                            .status(StatusCode::BAD_GATEWAY)
                            .body(Full::new(Bytes::from("Bad Gateway")))
                            .unwrap());
                    }
                };

                let mut response_builder = Response::builder().status(status.as_u16());

                for (name, value) in headers.iter() {
                    if name != "connection"
                        && name != "proxy-connection"
                        && name != "te"
                        && name != "trailers"
                        && name != "upgrade"
                    {
                        response_builder = response_builder.header(name, value);
                    }
                }

                Ok(response_builder.body(Full::new(body_bytes)).unwrap())
            }
            Err(e) => {
                error!("Failed to proxy request to upstream {}: {}", upstream.id, e);
                Ok(Response::builder()
                    .status(StatusCode::BAD_GATEWAY)
                    .body(Full::new(Bytes::from("Bad Gateway")))
                    .unwrap())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        HealthCheckConfig, LoadBalancerConfig, LoadBalancerStrategy, ProxyConfig, UpstreamConfig,
    };

    #[tokio::test]
    async fn test_reverse_proxy_creation() {
        let config = Config {
            proxy: ProxyConfig {
                bind_address: "127.0.0.1".to_string(),
                port: 8080,
            },
            load_balancer: LoadBalancerConfig {
                strategy: LoadBalancerStrategy::RoundRobin,
                health_check: Some(HealthCheckConfig {
                    interval_seconds: 30,
                    timeout_seconds: 5,
                    expected_status: 200,
                }),
            },
            upstreams: vec![UpstreamConfig {
                id: "test".to_string(),
                host: "127.0.0.1".to_string(),
                port: 3000,
                weight: 1,
                health_check_path: Some("/health".to_string()),
            }],
            routes: None,
        };

        let proxy = ReverseProxy::new(config).await;
        assert!(proxy.is_ok());
    }

    #[tokio::test]
    async fn test_proxy_request_invalid_upstream() {
        let config = Config {
            proxy: ProxyConfig {
                bind_address: "127.0.0.1".to_string(),
                port: 8080,
            },
            load_balancer: LoadBalancerConfig {
                strategy: LoadBalancerStrategy::RoundRobin,
                health_check: Some(HealthCheckConfig {
                    interval_seconds: 30,
                    timeout_seconds: 5,
                    expected_status: 200,
                }),
            },
            upstreams: vec![UpstreamConfig {
                id: "test".to_string(),
                host: "127.0.0.1".to_string(),
                port: 65535,
                weight: 1,
                health_check_path: Some("/health".to_string()),
            }],
            routes: None,
        };

        let _proxy = ReverseProxy::new(config).await.unwrap();
    }
}
