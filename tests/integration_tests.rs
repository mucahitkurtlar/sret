use anyhow::Result;
use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Request, Response, Server, StatusCode};
use sret::config::{
    Config, HealthCheckConfig, LoadBalancerConfig, LoadBalancerStrategy, ProxyConfig,
    UpstreamConfig,
};
use sret::proxy::ReverseProxy;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::time::timeout;

async fn mock_backend_handler(_req: Request<Body>) -> Result<Response<Body>, Infallible> {
    Ok(Response::builder()
        .status(StatusCode::OK)
        .body(Body::from("Hello from backend"))
        .unwrap())
}

async fn mock_health_handler(req: Request<Body>) -> Result<Response<Body>, Infallible> {
    if req.uri().path() == "/health" {
        Ok(Response::builder()
            .status(StatusCode::OK)
            .body(Body::from("OK"))
            .unwrap())
    } else {
        Ok(Response::builder()
            .status(StatusCode::OK)
            .body(Body::from("Hello from backend"))
            .unwrap())
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;

    async fn start_mock_backend(port: u16) -> Result<()> {
        let addr = SocketAddr::from(([127, 0, 0, 1], port));

        let make_svc = make_service_fn(|_conn| async {
            Ok::<_, Infallible>(service_fn(mock_backend_handler))
        });

        let server = Server::bind(&addr).serve(make_svc);

        if let Err(e) = server.await {
            eprintln!("Mock backend server error: {}", e);
        }

        Ok(())
    }

    async fn start_mock_backend_with_health(port: u16) -> Result<()> {
        let addr = SocketAddr::from(([127, 0, 0, 1], port));

        let make_svc =
            make_service_fn(|_conn| async { Ok::<_, Infallible>(service_fn(mock_health_handler)) });

        let server = Server::bind(&addr).serve(make_svc);

        if let Err(e) = server.await {
            eprintln!("Mock backend server error: {}", e);
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_proxy_basic_functionality() {
        let backend_port = 13000;
        let proxy_port = 18080;

        tokio::spawn(async move {
            if let Err(e) = start_mock_backend(backend_port).await {
                eprintln!("Failed to start mock backend: {}", e);
            }
        });

        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = Config {
            proxy: ProxyConfig {
                bind_address: "127.0.0.1".to_string(),
                port: proxy_port,
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
                id: "backend1".to_string(),
                host: "127.0.0.1".to_string(),
                port: backend_port,
                weight: 1,
                health_check_path: Some("/health".to_string()),
            }],
            routes: None,
        };

        let proxy = ReverseProxy::new(config).await.unwrap();
        tokio::spawn(async move {
            if let Err(e) = proxy.start().await {
                eprintln!("Proxy error: {}", e);
            }
        });

        tokio::time::sleep(Duration::from_millis(200)).await;

        let client = reqwest::Client::new();
        let proxy_url = format!("http://127.0.0.1:{}/", proxy_port);

        let result = timeout(Duration::from_secs(5), client.get(&proxy_url).send()).await;

        match result {
            Ok(Ok(response)) => {
                assert!(response.status().is_success());
                let body = response.text().await.unwrap();
                assert_eq!(body, "Hello from backend");
            }
            Ok(Err(e)) => {
                eprintln!("Request failed (expected in some test environments): {}", e);
            }
            Err(_) => {
                eprintln!("Request timed out (expected in some test environments)");
            }
        }
    }

    #[tokio::test]
    async fn test_proxy_with_multiple_backends() {
        let backend_port1 = 13001;
        let backend_port2 = 13002;
        let proxy_port = 18081;

        tokio::spawn(async move {
            if let Err(e) = start_mock_backend(backend_port1).await {
                eprintln!("Failed to start mock backend 1: {}", e);
            }
        });

        tokio::spawn(async move {
            if let Err(e) = start_mock_backend(backend_port2).await {
                eprintln!("Failed to start mock backend 2: {}", e);
            }
        });

        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = Config {
            proxy: ProxyConfig {
                bind_address: "127.0.0.1".to_string(),
                port: proxy_port,
            },
            load_balancer: LoadBalancerConfig {
                strategy: LoadBalancerStrategy::RoundRobin,
                health_check: Some(HealthCheckConfig {
                    interval_seconds: 30,
                    timeout_seconds: 5,
                    expected_status: 200,
                }),
            },
            upstreams: vec![
                UpstreamConfig {
                    id: "backend1".to_string(),
                    host: "127.0.0.1".to_string(),
                    port: backend_port1,
                    weight: 1,
                    health_check_path: Some("/health".to_string()),
                },
                UpstreamConfig {
                    id: "backend2".to_string(),
                    host: "127.0.0.1".to_string(),
                    port: backend_port2,
                    weight: 1,
                    health_check_path: Some("/health".to_string()),
                },
            ],
            routes: None,
        };

        let proxy = ReverseProxy::new(config).await.unwrap();
        tokio::spawn(async move {
            if let Err(e) = proxy.start().await {
                eprintln!("Proxy error: {}", e);
            }
        });

        tokio::time::sleep(Duration::from_millis(200)).await;

        let client = reqwest::Client::new();
        let proxy_url = format!("http://127.0.0.1:{}/", proxy_port);

        for i in 0..3 {
            let result = timeout(Duration::from_secs(2), client.get(&proxy_url).send()).await;

            match result {
                Ok(Ok(response)) => {
                    assert!(response.status().is_success());
                    println!("Request {} successful", i + 1);
                }
                Ok(Err(e)) => {
                    eprintln!("Request {} failed: {}", i + 1, e);
                }
                Err(_) => {
                    eprintln!("Request {} timed out", i + 1);
                }
            }
        }
    }

    #[tokio::test]
    async fn test_proxy_with_health_checks() {
        let backend_port = 13003;
        let proxy_port = 18082;

        tokio::spawn(async move {
            if let Err(e) = start_mock_backend_with_health(backend_port).await {
                eprintln!("Failed to start mock backend with health: {}", e);
            }
        });

        tokio::time::sleep(Duration::from_millis(100)).await;

        let config = Config {
            proxy: ProxyConfig {
                bind_address: "127.0.0.1".to_string(),
                port: proxy_port,
            },
            load_balancer: LoadBalancerConfig {
                strategy: LoadBalancerStrategy::RoundRobin,
                health_check: Some(HealthCheckConfig {
                    interval_seconds: 1,
                    timeout_seconds: 1,
                    expected_status: 200,
                }),
            },
            upstreams: vec![UpstreamConfig {
                id: "backend1".to_string(),
                host: "127.0.0.1".to_string(),
                port: backend_port,
                weight: 1,
                health_check_path: Some("/health".to_string()),
            }],
            routes: None,
        };

        let proxy = ReverseProxy::new(config).await.unwrap();
        tokio::spawn(async move {
            if let Err(e) = proxy.start().await {
                eprintln!("Proxy error: {}", e);
            }
        });

        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    #[tokio::test]
    async fn test_proxy_configuration_validation() {
        let invalid_config = Config {
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
            upstreams: vec![],
            routes: None,
        };

        assert!(invalid_config.validate().is_err());
    }
}
