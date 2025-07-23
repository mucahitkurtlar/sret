use hyper::StatusCode;
use sret::config::{Config, HealthCheckConfig, LoadBalancerStrategy};
use sret::server::Server;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::time::sleep;
use tracing::info;

const BACKEND_PORT_1: u16 = 19080;
const BACKEND_PORT_2: u16 = 19081;
const BACKEND_PORT_3: u16 = 19082;
const BACKEND_PORT_CONCURRENT_1: u16 = 19090;
const BACKEND_PORT_CONCURRENT_2: u16 = 19091;
const BACKEND_PORT_PATH_TEST: u16 = 19100;
const BACKEND_PORT_DOMAIN_TEST: u16 = 19110;
const PROXY_PORT_1: u16 = 19000;
const PROXY_PORT_CONCURRENT: u16 = 19001;
const PROXY_PORT_PATH_TEST: u16 = 19002;
const PROXY_PORT_DOMAIN_TEST: u16 = 19003;
const PROXY_PORT_ERROR_TEST: u16 = 19004;

const SERVER_STARTUP_DELAY_MS: u64 = 100;
const PROXY_STARTUP_DELAY_MS: u64 = 200;
const MAX_ROUTING_LATENCY_MS: u64 = 50;
const MAX_LOAD_BALANCING_LATENCY_MS: u64 = 100;
const MAX_ERROR_RESPONSE_LATENCY_MS: u64 = 30;
const MAX_CONCURRENT_TEST_DURATION_MS: u64 = 500;

const CONCURRENT_REQUEST_COUNT: usize = 20;
const LOAD_BALANCE_REQUEST_COUNT: usize = 6;
const ERROR_RESPONSE_TEST_COUNT: usize = 10;

fn create_performance_config() -> Config {
    Config {
        upstreams: vec![sret::config::UpstreamConfig {
            id: "backend".to_string(),
            targets: vec![
                sret::config::TargetConfig {
                    host: "127.0.0.1".to_string(),
                    port: BACKEND_PORT_1,
                    weight: Some(1),
                    health_check_path: Some("/health".to_string()),
                },
                sret::config::TargetConfig {
                    host: "127.0.0.1".to_string(),
                    port: BACKEND_PORT_2,
                    weight: Some(1),
                    health_check_path: Some("/health".to_string()),
                },
                sret::config::TargetConfig {
                    host: "127.0.0.1".to_string(),
                    port: BACKEND_PORT_3,
                    weight: Some(1),
                    health_check_path: Some("/health".to_string()),
                },
            ],
            strategy: Some(LoadBalancerStrategy::RoundRobin),
            health_check: Some(HealthCheckConfig {
                interval_seconds: 10,
                timeout_seconds: 2,
                expected_status: 200,
            }),
        }],
        servers: vec![sret::config::ServerConfig {
            id: "perf-test-server".to_string(),
            bind_address: "127.0.0.1".to_string(),
            port: PROXY_PORT_1,
            routes: vec![
                sret::config::RouteConfig {
                    domains: Some(vec!["api.perf.local".to_string()]),
                    paths: None,
                    upstream: "backend".to_string(),
                },
                sret::config::RouteConfig {
                    domains: None,
                    paths: Some(vec![
                        "/api".to_string(),
                        "/v1".to_string(),
                        "/v2".to_string(),
                    ]),
                    upstream: "backend".to_string(),
                },
            ],
        }],
    }
}

async fn start_fast_mock_backend(port: u16) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
        let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
        info!("Fast mock backend listening on {}", addr);

        loop {
            if let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut buffer = [0; 1024];
                    if stream.readable().await.is_ok() && stream.try_read(&mut buffer).is_ok() {
                        let response = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nOK";
                        let _ = stream.try_write(response.as_bytes());
                    }
                });
            }
        }
    })
}

async fn make_perf_request(
    url: &str,
    host_header: Option<&str>,
) -> Result<Duration, Box<dyn std::error::Error + Send + Sync>> {
    let client = reqwest::Client::new();
    let mut request_builder = client.get(url);

    if let Some(host) = host_header {
        request_builder = request_builder.header("Host", host);
    }

    let start = Instant::now();
    let response = request_builder.send().await?;
    let _ = response.text().await?;
    let duration = start.elapsed();

    Ok(duration)
}

#[tokio::test]
async fn test_load_balancer_round_robin() {
    let _ = tracing_subscriber::fmt::try_init();

    let _backend1 = start_fast_mock_backend(BACKEND_PORT_1).await;
    let _backend2 = start_fast_mock_backend(BACKEND_PORT_2).await;
    let _backend3 = start_fast_mock_backend(BACKEND_PORT_3).await;

    sleep(Duration::from_millis(SERVER_STARTUP_DELAY_MS)).await;

    let config = create_performance_config();
    let server_config = &config.servers[0];
    let server = Server::new(&config, server_config);
    let server = Arc::new(server);
    let server_clone = server.clone();

    let _proxy_handle = tokio::spawn(async move {
        server_clone.start().await.unwrap();
    });

    sleep(Duration::from_millis(PROXY_STARTUP_DELAY_MS)).await;

    let mut results = Vec::new();
    for _ in 0..LOAD_BALANCE_REQUEST_COUNT {
        let duration =
            make_perf_request(&format!("http://127.0.0.1:{}/api/test", PROXY_PORT_1), None)
                .await
                .unwrap();
        results.push(duration);
        sleep(Duration::from_millis(10)).await;
    }

    assert_eq!(results.len(), LOAD_BALANCE_REQUEST_COUNT);

    let avg_duration: Duration = results.iter().sum::<Duration>() / results.len() as u32;
    assert!(avg_duration < Duration::from_millis(MAX_LOAD_BALANCING_LATENCY_MS));

    println!("Load balancer round-robin test passed");
    println!("   Average response time: {:?}", avg_duration);
}

#[tokio::test]
async fn test_concurrent_requests() {
    let _ = tracing_subscriber::fmt::try_init();

    let _backend1 = start_fast_mock_backend(BACKEND_PORT_CONCURRENT_1).await;
    let _backend2 = start_fast_mock_backend(BACKEND_PORT_CONCURRENT_2).await;

    sleep(Duration::from_millis(SERVER_STARTUP_DELAY_MS)).await;

    let mut config = create_performance_config();
    config.servers[0].port = PROXY_PORT_CONCURRENT;
    config.upstreams[0].targets[0].port = BACKEND_PORT_CONCURRENT_1;
    config.upstreams[0].targets[1].port = BACKEND_PORT_CONCURRENT_2;
    config.upstreams[0].targets.truncate(2);

    let server_config = &config.servers[0];
    let server = Server::new(&config, server_config);
    let server = Arc::new(server);
    let server_clone = server.clone();

    let _proxy_handle = tokio::spawn(async move {
        server_clone.start().await.unwrap();
    });

    sleep(Duration::from_millis(PROXY_STARTUP_DELAY_MS)).await;

    let start_time = Instant::now();
    let mut handles = Vec::new();

    for i in 0..CONCURRENT_REQUEST_COUNT {
        let handle = tokio::spawn(async move {
            make_perf_request(
                &format!(
                    "http://127.0.0.1:{}/api/request_{}",
                    PROXY_PORT_CONCURRENT, i
                ),
                None,
            )
            .await
        });
        handles.push(handle);
    }

    let mut results = Vec::new();
    for handle in handles {
        match handle.await.unwrap() {
            Ok(duration) => results.push(duration),
            Err(e) => panic!("Request failed: {}", e),
        }
    }

    let total_time = start_time.elapsed();

    assert_eq!(results.len(), CONCURRENT_REQUEST_COUNT);

    assert!(total_time < Duration::from_millis(MAX_CONCURRENT_TEST_DURATION_MS));

    let avg_duration: Duration = results.iter().sum::<Duration>() / results.len() as u32;

    println!("Concurrent requests test passed");
    println!(
        "   Total time for {} requests: {:?}",
        CONCURRENT_REQUEST_COUNT, total_time
    );
    println!("   Average individual request time: {:?}", avg_duration);
}

#[tokio::test]
async fn test_path_routing_performance() {
    let _ = tracing_subscriber::fmt::try_init();

    let _backend = start_fast_mock_backend(BACKEND_PORT_PATH_TEST).await;

    sleep(Duration::from_millis(SERVER_STARTUP_DELAY_MS)).await;

    let mut config = create_performance_config();
    config.servers[0].port = PROXY_PORT_PATH_TEST;
    config.upstreams[0].targets[0].port = BACKEND_PORT_PATH_TEST;
    config.upstreams[0].targets.truncate(1);

    config.servers[0].routes.extend(vec![
        sret::config::RouteConfig {
            domains: None,
            paths: Some(vec!["/health".to_string()]),
            upstream: "backend".to_string(),
        },
        sret::config::RouteConfig {
            domains: None,
            paths: Some(vec!["/metrics".to_string()]),
            upstream: "backend".to_string(),
        },
        sret::config::RouteConfig {
            domains: None,
            paths: Some(vec!["/status".to_string()]),
            upstream: "backend".to_string(),
        },
    ]);

    let server_config = &config.servers[0];
    let server = Server::new(&config, server_config);
    let server = Arc::new(server);
    let server_clone = server.clone();

    let _proxy_handle = tokio::spawn(async move {
        server_clone.start().await.unwrap();
    });

    sleep(Duration::from_millis(PROXY_STARTUP_DELAY_MS)).await;

    let paths = vec![
        "/api/users",
        "/v1/data",
        "/v2/info",
        "/health",
        "/metrics",
        "/status",
    ];
    let mut total_duration = Duration::ZERO;

    for path in &paths {
        let url = format!("http://127.0.0.1:{}{}", PROXY_PORT_PATH_TEST, path);
        let duration = make_perf_request(&url, None).await.unwrap();
        total_duration += duration;
    }

    let avg_duration = total_duration / paths.len() as u32;

    assert!(avg_duration < Duration::from_millis(MAX_ROUTING_LATENCY_MS));

    println!("Path routing performance test passed");
    println!("   Average routing time: {:?}", avg_duration);
}

#[tokio::test]
async fn test_domain_routing_performance() {
    let _ = tracing_subscriber::fmt::try_init();

    let _backend = start_fast_mock_backend(BACKEND_PORT_DOMAIN_TEST).await;

    sleep(Duration::from_millis(SERVER_STARTUP_DELAY_MS)).await;

    let mut config = create_performance_config();
    config.servers[0].port = PROXY_PORT_DOMAIN_TEST;
    config.upstreams[0].targets[0].port = BACKEND_PORT_DOMAIN_TEST;
    config.upstreams[0].targets.truncate(1);

    let server_config = &config.servers[0];
    let server = Server::new(&config, server_config);
    let server = Arc::new(server);
    let server_clone = server.clone();

    let _proxy_handle = tokio::spawn(async move {
        server_clone.start().await.unwrap();
    });

    sleep(Duration::from_millis(PROXY_STARTUP_DELAY_MS)).await;

    let mut results = Vec::new();
    for _ in 0..ERROR_RESPONSE_TEST_COUNT {
        let duration = make_perf_request(
            &format!("http://127.0.0.1:{}/test", PROXY_PORT_DOMAIN_TEST),
            Some("api.perf.local"),
        )
        .await
        .unwrap();
        results.push(duration);
    }

    let avg_duration: Duration = results.iter().sum::<Duration>() / results.len() as u32;

    assert!(avg_duration < Duration::from_millis(MAX_ROUTING_LATENCY_MS));

    println!("Domain routing performance test passed");
    println!("   Average domain routing time: {:?}", avg_duration);
}

#[tokio::test]
async fn test_error_handling_performance() {
    let _ = tracing_subscriber::fmt::try_init();

    let mut config = create_performance_config();
    config.servers[0].port = PROXY_PORT_ERROR_TEST;

    let server_config = &config.servers[0];
    let server = Server::new(&config, server_config);
    let server = Arc::new(server);
    let server_clone = server.clone();

    let _proxy_handle = tokio::spawn(async move {
        server_clone.start().await.unwrap();
    });

    sleep(Duration::from_millis(PROXY_STARTUP_DELAY_MS)).await;

    let client = reqwest::Client::new();
    let mut results = Vec::new();

    for _ in 0..ERROR_RESPONSE_TEST_COUNT {
        let start = Instant::now();
        let response = client
            .get(format!(
                "http://127.0.0.1:{}/unknown",
                PROXY_PORT_ERROR_TEST
            ))
            .send()
            .await
            .unwrap();
        let status = response.status();
        let _ = response.text().await.unwrap();
        let duration = start.elapsed();

        assert_eq!(status, StatusCode::NOT_FOUND);
        results.push(duration);
    }

    let avg_duration: Duration = results.iter().sum::<Duration>() / results.len() as u32;

    assert!(avg_duration < Duration::from_millis(MAX_ERROR_RESPONSE_LATENCY_MS));

    println!("Error handling performance test passed");
    println!("   Average 404 response time: {:?}", avg_duration);
}
