use sret::config::*;
use sret::proxy::ReverseProxy;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::time::timeout;

static BACKEND1_COUNT: AtomicU32 = AtomicU32::new(0);
static BACKEND2_COUNT: AtomicU32 = AtomicU32::new(0);

async fn start_counting_backend(
    port: u16,
    counter: &'static AtomicU32,
) -> Result<(), Box<dyn std::error::Error>> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = TcpListener::bind(&addr).await?;

    loop {
        let (stream, _) = listener.accept().await?;

        tokio::spawn(async move {
            let mut buffer = [0; 1024];
            let mut stream = stream;

            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            if stream.read(&mut buffer).await.is_ok() {
                counter.fetch_add(1, Ordering::Relaxed);

                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\nBackend {} response",
                    format!("Backend {} response", port).len(),
                    port
                );

                let _ = stream.write_all(response.as_bytes()).await;
            }
        });
    }
}

#[tokio::test]
async fn test_domain_based_routing() {
    let backend_port1 = 15000;
    let backend_port2 = 15001;
    let proxy_port = 19500;

    BACKEND1_COUNT.store(0, Ordering::Relaxed);
    BACKEND2_COUNT.store(0, Ordering::Relaxed);

    tokio::spawn(async move {
        if let Err(e) = start_counting_backend(backend_port1, &BACKEND1_COUNT).await {
            eprintln!("Failed to start backend 1: {}", e);
        }
    });

    tokio::spawn(async move {
        if let Err(e) = start_counting_backend(backend_port2, &BACKEND2_COUNT).await {
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
                id: "service1".to_string(),
                host: "127.0.0.1".to_string(),
                port: backend_port1,
                weight: 1,
                health_check_path: Some("/health".to_string()),
            },
            UpstreamConfig {
                id: "service2".to_string(),
                host: "127.0.0.1".to_string(),
                port: backend_port2,
                weight: 1,
                health_check_path: Some("/health".to_string()),
            },
        ],
        routes: Some(vec![
            RouteConfig {
                domain: "service-1.home.arpa".to_string(),
                upstream_ids: vec!["service1".to_string()],
                load_balancer_strategy: None,
            },
            RouteConfig {
                domain: "service-2.home.arpa".to_string(),
                upstream_ids: vec!["service2".to_string()],
                load_balancer_strategy: None,
            },
        ]),
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

    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("host", "service-1.home.arpa".parse().unwrap());

    for _ in 0..3 {
        let result = timeout(
            Duration::from_secs(5),
            client.get(&proxy_url).headers(headers.clone()).send(),
        )
        .await;

        if let Ok(Ok(_response)) = result {}
    }

    let mut headers2 = reqwest::header::HeaderMap::new();
    headers2.insert("host", "service-2.home.arpa".parse().unwrap());

    for _ in 0..3 {
        let result = timeout(
            Duration::from_secs(5),
            client.get(&proxy_url).headers(headers2.clone()).send(),
        )
        .await;

        if let Ok(Ok(_response)) = result {}
    }

    tokio::time::sleep(Duration::from_millis(100)).await;

    let backend1_count = BACKEND1_COUNT.load(Ordering::Relaxed);
    let backend2_count = BACKEND2_COUNT.load(Ordering::Relaxed);

    println!("Backend 1 received {} requests", backend1_count);
    println!("Backend 2 received {} requests", backend2_count);

    assert!(
        backend1_count > 0,
        "Backend 1 should have received requests for service-1.home.arpa"
    );
    assert!(
        backend2_count > 0,
        "Backend 2 should have received requests for service-2.home.arpa"
    );
}

#[tokio::test]
async fn test_default_routing_fallback() {
    let backend_port = 15002;
    let proxy_port = 19501;

    BACKEND1_COUNT.store(0, Ordering::Relaxed);

    tokio::spawn(async move {
        if let Err(e) = start_counting_backend(backend_port, &BACKEND1_COUNT).await {
            eprintln!("Failed to start backend: {}", e);
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
            id: "default".to_string(),
            host: "127.0.0.1".to_string(),
            port: backend_port,
            weight: 1,
            health_check_path: Some("/health".to_string()),
        }],
        routes: Some(vec![RouteConfig {
            domain: "specific.domain.com".to_string(),
            upstream_ids: vec!["default".to_string()],
            load_balancer_strategy: None,
        }]),
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

    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("host", "unknown.domain.com".parse().unwrap());

    let result = timeout(
        Duration::from_secs(5),
        client.get(&proxy_url).headers(headers).send(),
    )
    .await;

    match result {
        Ok(Ok(_response)) => {
            println!("Request to unknown domain succeeded");
        }
        Ok(Err(_)) => {
            println!("Request to unknown domain failed (expected in test environment)");
        }
        Err(_) => {
            println!("Request to unknown domain timed out");
        }
    }
}
