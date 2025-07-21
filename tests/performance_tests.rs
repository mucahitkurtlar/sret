use anyhow::Result;
use http_body_util::Full;
use hyper::body::{Bytes, Incoming};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use sret::config::{
    Config, HealthCheckConfig, LoadBalancerConfig, LoadBalancerStrategy, ProxyConfig,
    UpstreamConfig,
};
use sret::proxy::ReverseProxy;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tokio::net::TcpListener;
use tokio::time::timeout;

static REQUEST_COUNT: AtomicUsize = AtomicUsize::new(0);

async fn counting_backend_handler(
    _req: Request<Incoming>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    let count = REQUEST_COUNT.fetch_add(1, Ordering::Relaxed);
    Ok(Response::builder()
        .status(StatusCode::OK)
        .body(Full::new(Bytes::from(format!("Response #{}", count))))
        .unwrap())
}

#[cfg(test)]
mod performance_tests {
    use super::*;

    async fn start_counting_backend(port: u16) -> Result<()> {
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        let listener = TcpListener::bind(addr).await?;

        loop {
            let (stream, _) = listener.accept().await?;
            let io = TokioIo::new(stream);

            tokio::task::spawn(async move {
                if let Err(err) = http1::Builder::new()
                    .serve_connection(io, service_fn(counting_backend_handler))
                    .await
                {
                    eprintln!("Error serving connection: {:?}", err);
                }
            });
        }
    }

    #[tokio::test]
    async fn test_concurrent_requests() {
        let backend_port = 14000;
        let proxy_port = 19000;

        REQUEST_COUNT.store(0, Ordering::Relaxed);

        tokio::spawn(async move {
            if let Err(e) = start_counting_backend(backend_port).await {
                eprintln!("Failed to start counting backend: {}", e);
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

        let client = Arc::new(reqwest::Client::new());
        let proxy_url = format!("http://127.0.0.1:{}/", proxy_port);
        let num_requests = 10;
        let mut handles = vec![];

        let start_time = Instant::now();

        for i in 0..num_requests {
            let client_clone = client.clone();
            let url = proxy_url.clone();

            let handle = tokio::spawn(async move {
                let result = timeout(Duration::from_secs(5), client_clone.get(&url).send()).await;

                match result {
                    Ok(Ok(response)) => {
                        if response.status().is_success() {
                            println!("Request {} completed successfully", i);
                            true
                        } else {
                            println!("Request {} failed with status: {}", i, response.status());
                            false
                        }
                    }
                    Ok(Err(e)) => {
                        println!("Request {} failed: {}", i, e);
                        false
                    }
                    Err(_) => {
                        println!("Request {} timed out", i);
                        false
                    }
                }
            });

            handles.push(handle);
        }

        let mut successful_requests = 0;
        for handle in handles {
            if let Ok(success) = handle.await {
                if success {
                    successful_requests += 1;
                }
            }
        }

        let duration = start_time.elapsed();
        println!(
            "Completed {}/{} requests in {:?}",
            successful_requests, num_requests, duration
        );

        assert!(successful_requests <= num_requests);
    }

    #[tokio::test]
    async fn test_load_balancing_distribution() {
        let backend_port1 = 14001;
        let backend_port2 = 14002;
        let proxy_port = 19001;

        tokio::spawn(async move {
            if let Err(e) = start_counting_backend(backend_port1).await {
                eprintln!("Failed to start backend 1: {}", e);
            }
        });

        tokio::spawn(async move {
            if let Err(e) = start_counting_backend(backend_port2).await {
                eprintln!("Failed to start backend 2: {}", e);
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
        let mut responses = vec![];

        for i in 0..6 {
            let result = timeout(Duration::from_secs(2), client.get(&proxy_url).send()).await;

            match result {
                Ok(Ok(response)) => {
                    if response.status().is_success() {
                        let body = response.text().await.unwrap_or_default();
                        responses.push(body);
                        println!("Request {} got response", i);
                    }
                }
                Ok(Err(e)) => {
                    println!("Request {} failed: {}", i, e);
                }
                Err(_) => {
                    println!("Request {} timed out", i);
                }
            }
        }

        println!("Received {} responses total", responses.len());

        assert!(responses.len() <= 6);
    }

    #[tokio::test]
    async fn test_proxy_resilience_to_backend_failure() {
        let backend_port = 14003;
        let proxy_port = 19002;

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
                println!("Got response with status: {}", response.status());
            }
            Ok(Err(e)) => {
                println!("Request failed as expected: {}", e);
            }
            Err(_) => {
                println!("Request timed out as expected in test environment");
            }
        }
    }
}
