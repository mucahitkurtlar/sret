use hyper::{Method, StatusCode};
use sret::config::{Config, HealthCheckConfig, LoadBalancerStrategy};
use sret::server::Server;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::time::sleep;
use tracing::{info, warn};

const BACKEND_PORT_1: u16 = 18080;
const BACKEND_PORT_2: u16 = 18081;
const BACKEND_PORT_HEALTH: u16 = 18085;
const CONSOLE_PORT: u16 = 13000;
const CONSOLE_PORT_TEST: u16 = 33000;

const PATH_TEST_BACKEND_1: u16 = 28080;
const PATH_TEST_BACKEND_2: u16 = 28081;
const POST_TEST_BACKEND: u16 = 48080;

const DEFAULT_PROXY_PORT: u16 = 18000;
const DOMAIN_UNMATCHED_PROXY_PORT: u16 = 18005;
const HEALTH_CHECK_PROXY_PORT: u16 = 18006;
const PATH_TEST_PROXY_PORT: u16 = 28000;
const CONSOLE_TEST_PROXY_PORT: u16 = 38000;
const POST_TEST_PROXY_PORT: u16 = 48000;
const UNMATCHED_ROUTE_PROXY_PORT: u16 = 58000;

const BACKEND_STARTUP_DELAY_MS: u64 = 100;
const PROXY_STARTUP_DELAY_MS: u64 = 200;

fn create_test_config() -> Config {
    Config {
        upstreams: vec![
            sret::config::UpstreamConfig {
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
                        weight: Some(2),
                        health_check_path: Some("/health".to_string()),
                    },
                ],
                strategy: Some(LoadBalancerStrategy::RoundRobin),
                health_check: Some(HealthCheckConfig {
                    interval_seconds: 30,
                    timeout_seconds: 5,
                    expected_status: 200,
                }),
            },
            sret::config::UpstreamConfig {
                id: "console".to_string(),
                targets: vec![sret::config::TargetConfig {
                    host: "127.0.0.1".to_string(),
                    port: CONSOLE_PORT,
                    weight: Some(1),
                    health_check_path: None,
                }],
                strategy: None,
                health_check: None,
            },
        ],
        servers: vec![sret::config::ServerConfig {
            id: "test-server".to_string(),
            bind_address: "127.0.0.1".to_string(),
            port: DEFAULT_PROXY_PORT,
            tls: None,
            routes: vec![
                sret::config::RouteConfig {
                    domains: Some(vec!["api.test.local".to_string()]),
                    paths: None,
                    upstream: "backend".to_string(),
                },
                sret::config::RouteConfig {
                    domains: None,
                    paths: Some(vec!["/api".to_string()]),
                    upstream: "backend".to_string(),
                },
                sret::config::RouteConfig {
                    domains: None,
                    paths: Some(vec!["/console".to_string()]),
                    upstream: "console".to_string(),
                },
            ],
        }],
    }
}

async fn start_mock_backend(port: u16) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        let listener = TcpListener::bind(addr).await.unwrap();
        info!("Mock backend server listening on {}", addr);

        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    tokio::spawn(async move {
                        let mut buffer = [0; 1024];
                        match stream.readable().await {
                            Ok(_) => {
                                if let Ok(n) = stream.try_read(&mut buffer) {
                                    let request = String::from_utf8_lossy(&buffer[..n]);

                                    let response = if request.contains("GET /health") {
                                        "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nOK"
                                    } else if request.contains("POST") {
                                        &format!(
                                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{{\"message\":\"Backend response from port {}\",\"method\":\"POST\"}}",
                                            format!("{{\"message\":\"Backend response from port {}\",\"method\":\"POST\"}}", port).len(),
                                            port
                                        )
                                    } else {
                                        &format!(
                                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{{\"message\":\"Backend response from port {}\",\"method\":\"GET\"}}",
                                            format!("{{\"message\":\"Backend response from port {}\",\"method\":\"GET\"}}", port).len(),
                                            port
                                        )
                                    };

                                    if let Err(e) = stream.try_write(response.as_bytes()) {
                                        warn!("Failed to write response: {}", e);
                                    }
                                }
                            }
                            Err(e) => warn!("Stream read error: {}", e),
                        }
                    });
                }
                Err(e) => warn!("Failed to accept connection: {}", e),
            }
        }
    })
}

async fn start_mock_console(port: u16) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        let listener = TcpListener::bind(addr).await.unwrap();
        info!("Mock console server listening on {}", addr);

        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    tokio::spawn(async move {
                        let mut buffer = [0; 1024];
                        match stream.readable().await {
                            Ok(_) => {
                                if stream.try_read(&mut buffer).is_ok() {
                                    let response = "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 15\r\n\r\nConsole service";
                                    if let Err(e) = stream.try_write(response.as_bytes()) {
                                        warn!("Failed to write response: {}", e);
                                    }
                                }
                            }
                            Err(e) => warn!("Stream read error: {}", e),
                        }
                    });
                }
                Err(e) => warn!("Failed to accept connection: {}", e),
            }
        }
    })
}

async fn make_request(
    method: Method,
    url: &str,
    host_header: Option<&str>,
    body: Option<&str>,
) -> Result<(StatusCode, String), Box<dyn std::error::Error + Send + Sync>> {
    let client = reqwest::Client::new();
    let mut request_builder = client.request(method, url);

    if let Some(host) = host_header {
        request_builder = request_builder.header("Host", host);
    }

    if let Some(body_content) = body {
        request_builder = request_builder
            .header("Content-Type", "application/json")
            .body(body_content.to_string());
    }

    let response = request_builder.send().await?;
    let status = response.status();
    let body_text = response.text().await?;

    Ok((status, body_text))
}

#[tokio::test]
async fn test_domain_based_routing() {
    let _ = tracing_subscriber::fmt::try_init();

    let _backend1 = start_mock_backend(BACKEND_PORT_1).await;
    let _backend2 = start_mock_backend(BACKEND_PORT_2).await;

    sleep(Duration::from_millis(BACKEND_STARTUP_DELAY_MS)).await;

    let config = create_test_config();
    let server_config = &config.servers[0];
    let server = Server::new(&config, server_config);
    let server = Arc::new(server);
    let server_clone = server.clone();

    let _proxy_handle = tokio::spawn(async move {
        server_clone.start().await.unwrap();
    });

    sleep(Duration::from_millis(PROXY_STARTUP_DELAY_MS)).await;

    let (status, body) = make_request(
        Method::GET,
        &format!("http://127.0.0.1:{}/", DEFAULT_PROXY_PORT),
        Some("api.test.local"),
        None,
    )
    .await
    .unwrap();

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Backend response from port"));
    assert!(
        body.contains(&BACKEND_PORT_1.to_string()) || body.contains(&BACKEND_PORT_2.to_string())
    );

    println!("Domain-based routing test passed");
}

#[tokio::test]
async fn test_path_based_routing() {
    tracing_subscriber::fmt::try_init().ok();

    let _backend1 = start_mock_backend(PATH_TEST_BACKEND_1).await;
    let _backend2 = start_mock_backend(PATH_TEST_BACKEND_2).await;

    sleep(Duration::from_millis(BACKEND_STARTUP_DELAY_MS)).await;

    let mut config = create_test_config();
    config.servers[0].port = PATH_TEST_PROXY_PORT;
    config.upstreams[0].targets[0].port = PATH_TEST_BACKEND_1;
    config.upstreams[0].targets[1].port = PATH_TEST_BACKEND_2;

    let server_config = &config.servers[0];
    let server = Server::new(&config, server_config);
    let server = Arc::new(server);
    let server_clone = server.clone();

    let _proxy_handle = tokio::spawn(async move {
        server_clone.start().await.unwrap();
    });

    sleep(Duration::from_millis(PROXY_STARTUP_DELAY_MS)).await;

    let (status, body) = make_request(
        Method::GET,
        &format!("http://127.0.0.1:{}/api/users", PATH_TEST_PROXY_PORT),
        None,
        None,
    )
    .await
    .unwrap();

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Backend response from port"));

    println!("Path-based routing test passed");
}

#[tokio::test]
async fn test_console_routing() {
    tracing_subscriber::fmt::try_init().ok();

    let _console = start_mock_console(CONSOLE_PORT_TEST).await;

    sleep(Duration::from_millis(BACKEND_STARTUP_DELAY_MS)).await;

    let mut config = create_test_config();
    config.servers[0].port = CONSOLE_TEST_PROXY_PORT;
    config.upstreams[1].targets[0].port = CONSOLE_PORT_TEST;

    let server_config = &config.servers[0];
    let server = Server::new(&config, server_config);
    let server = Arc::new(server);
    let server_clone = server.clone();

    let _proxy_handle = tokio::spawn(async move {
        server_clone.start().await.unwrap();
    });

    sleep(Duration::from_millis(PROXY_STARTUP_DELAY_MS)).await;

    let (status, body) = make_request(
        Method::GET,
        &format!("http://127.0.0.1:{}/console/admin", CONSOLE_TEST_PROXY_PORT),
        None,
        None,
    )
    .await
    .unwrap();

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "Console service");

    println!("Console routing test passed");
}

#[tokio::test]
async fn test_post_request_with_body() {
    tracing_subscriber::fmt::try_init().ok();

    let _backend = start_mock_backend(POST_TEST_BACKEND).await;

    sleep(Duration::from_millis(BACKEND_STARTUP_DELAY_MS)).await;

    let mut config = create_test_config();
    config.servers[0].port = POST_TEST_PROXY_PORT;
    config.upstreams[0].targets[0].port = POST_TEST_BACKEND;
    config.upstreams[0].targets[1].port = 48081;

    let server_config = &config.servers[0];
    let server = Server::new(&config, server_config);
    let server = Arc::new(server);
    let server_clone = server.clone();

    let _proxy_handle = tokio::spawn(async move {
        server_clone.start().await.unwrap();
    });

    sleep(Duration::from_millis(PROXY_STARTUP_DELAY_MS)).await;

    let (status, body) = make_request(
        Method::POST,
        &format!("http://127.0.0.1:{}/api/create", POST_TEST_PROXY_PORT),
        None,
        Some(r#"{"name": "test", "value": 123}"#),
    )
    .await
    .unwrap();

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Backend response"));
    assert!(body.contains("POST"));

    println!("POST request with body test passed");
}

#[tokio::test]
async fn test_unmatched_route_returns_404() {
    tracing_subscriber::fmt::try_init().ok();

    let mut config = create_test_config();
    config.servers[0].port = UNMATCHED_ROUTE_PROXY_PORT;

    let server_config = &config.servers[0];
    let server = Server::new(&config, server_config);
    let server = Arc::new(server);
    let server_clone = server.clone();

    let _proxy_handle = tokio::spawn(async move {
        server_clone.start().await.unwrap();
    });

    sleep(Duration::from_millis(PROXY_STARTUP_DELAY_MS)).await;

    let (status, body) = make_request(
        Method::GET,
        &format!(
            "http://127.0.0.1:{}/unknown/path",
            UNMATCHED_ROUTE_PROXY_PORT
        ),
        None,
        None,
    )
    .await
    .unwrap();

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body, "No matching route found");

    println!("Unmatched route 404 test passed");
}

#[tokio::test]
async fn test_unmatched_domain_returns_404() {
    tracing_subscriber::fmt::try_init().ok();

    let mut config = create_test_config();
    config.servers[0].port = DOMAIN_UNMATCHED_PROXY_PORT;

    let server_config = &config.servers[0];
    let server = Server::new(&config, server_config);
    let server = Arc::new(server);
    let server_clone = server.clone();

    let _proxy_handle = tokio::spawn(async move {
        server_clone.start().await.unwrap();
    });

    sleep(Duration::from_millis(PROXY_STARTUP_DELAY_MS)).await;

    let (status, body) = make_request(
        Method::GET,
        &format!("http://127.0.0.1:{}/", DOMAIN_UNMATCHED_PROXY_PORT),
        Some("unknown.domain.com"),
        None,
    )
    .await
    .unwrap();

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body, "No matching route found");

    println!("Unmatched domain 404 test passed");
}

#[tokio::test]
async fn test_health_check_endpoint() {
    tracing_subscriber::fmt::try_init().ok();

    let _backend = start_mock_backend(BACKEND_PORT_HEALTH).await;

    sleep(Duration::from_millis(BACKEND_STARTUP_DELAY_MS)).await;

    let mut config = create_test_config();
    config.servers[0].port = HEALTH_CHECK_PROXY_PORT;
    config.upstreams[0].targets[0].port = BACKEND_PORT_HEALTH;

    let server_config = &config.servers[0];
    let server = Server::new(&config, server_config);
    let server = Arc::new(server);
    let server_clone = server.clone();

    let _proxy_handle = tokio::spawn(async move {
        server_clone.start().await.unwrap();
    });

    sleep(Duration::from_millis(PROXY_STARTUP_DELAY_MS)).await;

    let (status, body) = make_request(
        Method::GET,
        &format!("http://127.0.0.1:{}/api/health", HEALTH_CHECK_PROXY_PORT),
        None,
        None,
    )
    .await
    .unwrap();

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "OK");

    println!("Health check endpoint test passed");
}
