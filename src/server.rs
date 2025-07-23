use crate::config::{Config, ServerConfig};
use crate::health_check::{HealthCheckManager, HealthChecker};
use crate::proxy::ProxyHandler;
use crate::router::Router;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use std::sync::Arc;
use tokio::net::TcpListener;
use tracing::{error, info, warn};

pub struct Server {
    pub id: String,
    pub bind_address: String,
    pub port: u16,
    pub router: Arc<Router>,
    pub proxy_handler: Arc<ProxyHandler>,
    pub health_manager: Option<HealthCheckManager>,
}

impl Server {
    pub fn new(config: &Config, server_config: &ServerConfig) -> Self {
        info!("Creating server: {}", server_config.id);

        let router = Arc::new(Router::new(config, server_config));
        let proxy_handler = Arc::new(ProxyHandler::new());

        let health_manager = if config.upstreams.iter().any(|u| u.health_check.is_some()) {
            let mut manager = HealthCheckManager::new();

            for upstream_config in &config.upstreams {
                if let Some(health_config) = &upstream_config.health_check {
                    let targets = if let Some(upstream) = router.get_upstream(&upstream_config.id) {
                        upstream.all_targets().to_vec()
                    } else {
                        Vec::new()
                    };

                    if !targets.is_empty() {
                        let checker = HealthChecker::new(health_config.clone(), targets);
                        manager.add_checker(checker);
                    }
                }
            }

            Some(manager)
        } else {
            None
        };

        Self {
            id: server_config.id.clone(),
            bind_address: server_config.bind_address.clone(),
            port: server_config.port,
            router,
            proxy_handler,
            health_manager,
        }
    }

    pub async fn start(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let addr = format!("{}:{}", self.bind_address, self.port);
        info!("Starting server {} on {}", self.id, addr);

        if let Some(manager) = self.health_manager.clone() {
            tokio::spawn(async move {
                manager.start_all().await;
            });
            info!("Health checks started for server {}", self.id);
        }

        let listener = TcpListener::bind(&addr).await?;
        info!("Server {} listening on {}", self.id, addr);

        let router = self.router.clone();
        let proxy_handler = self.proxy_handler.clone();
        let server_id = self.id.clone();

        loop {
            match listener.accept().await {
                Ok((stream, remote_addr)) => {
                    let router = router.clone();
                    let proxy_handler = proxy_handler.clone();
                    let server_id = server_id.clone();

                    tokio::spawn(async move {
                        let io = TokioIo::new(stream);

                        let service = service_fn(move |req| {
                            let router = router.clone();
                            let proxy_handler = proxy_handler.clone();
                            let server_id = server_id.clone();

                            async move { proxy_handler.handle_request(req, router, server_id).await }
                        });

                        if let Err(err) = http1::Builder::new().serve_connection(io, service).await
                        {
                            error!("Failed to serve connection from {}: {}", remote_addr, err);
                        }
                    });
                }
                Err(e) => {
                    error!("Failed to accept connection on server {}: {}", server_id, e);
                }
            }
        }
    }

    pub fn stats(&self) -> ServerStats {
        ServerStats {
            id: self.id.clone(),
            bind_address: self.bind_address.clone(),
            port: self.port,
            router_stats: self.router.stats(),
            has_health_checks: self.health_manager.is_some(),
        }
    }

    pub fn has_healthy_targets(&self) -> bool {
        self.router.has_healthy_targets()
    }

    // TODO: Implement graceful shutdown
    pub async fn shutdown(&self) {
        info!("Shutting down server {}", self.id);
        warn!("Graceful shutdown not fully implemented yet");
    }
}

#[derive(Debug)]
pub struct ServerStats {
    pub id: String,
    pub bind_address: String,
    pub port: u16,
    pub router_stats: crate::router::RouterStats,
    pub has_health_checks: bool,
}

impl std::fmt::Display for ServerStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Server Stats ({})", self.id)?;
        writeln!(f, "  Address: {}:{}", self.bind_address, self.port)?;
        writeln!(
            f,
            "  Health checks: {}",
            if self.has_health_checks {
                "enabled"
            } else {
                "disabled"
            }
        )?;
        writeln!(f, "  {}", self.router_stats)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{RouteConfig, TargetConfig, UpstreamConfig};

    fn create_test_config() -> (Config, ServerConfig) {
        let config = Config {
            upstreams: vec![UpstreamConfig {
                id: "backend".to_string(),
                targets: vec![TargetConfig {
                    host: "127.0.0.1".to_string(),
                    port: 3000,
                    weight: Some(1),
                    health_check_path: None,
                }],
                strategy: None,
                health_check: None,
            }],
            servers: vec![],
        };

        let server_config = ServerConfig {
            id: "test-server".to_string(),
            bind_address: "127.0.0.1".to_string(),
            port: 8080,
            routes: vec![RouteConfig {
                domains: None,
                paths: None,
                upstream: "backend".to_string(),
            }],
        };

        (config, server_config)
    }

    #[test]
    fn test_server_creation() {
        let (config, server_config) = create_test_config();
        let server = Server::new(&config, &server_config);

        assert_eq!(server.id, "test-server");
        assert_eq!(server.bind_address, "127.0.0.1");
        assert_eq!(server.port, 8080);
        assert!(server.has_healthy_targets());
    }

    #[test]
    fn test_server_stats() {
        let (config, server_config) = create_test_config();
        let server = Server::new(&config, &server_config);

        let stats = server.stats();
        assert_eq!(stats.id, "test-server");
        assert_eq!(stats.port, 8080);
        assert!(!stats.has_health_checks);
    }

    #[test]
    fn test_server_with_health_checks() {
        let config = Config {
            upstreams: vec![UpstreamConfig {
                id: "backend".to_string(),
                targets: vec![TargetConfig {
                    host: "127.0.0.1".to_string(),
                    port: 3000,
                    weight: Some(1),
                    health_check_path: Some("/health".to_string()),
                }],
                strategy: None,
                health_check: Some(crate::config::HealthCheckConfig {
                    interval_seconds: 30,
                    timeout_seconds: 5,
                    expected_status: 200,
                }),
            }],
            servers: vec![],
        };

        let server_config = ServerConfig {
            id: "test-server".to_string(),
            bind_address: "127.0.0.1".to_string(),
            port: 8080,
            routes: vec![RouteConfig {
                domains: None,
                paths: None,
                upstream: "backend".to_string(),
            }],
        };

        let server = Server::new(&config, &server_config);
        let stats = server.stats();
        assert!(stats.has_health_checks);
    }
}
