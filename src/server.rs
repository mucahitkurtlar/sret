use crate::config::{Config, ServerConfig};
use crate::health_check::{HealthCheckManager, HealthChecker};
use crate::proxy::ProxyHandler;
use crate::router::Router;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use rustls::ServerConfig as RustlsConfig;
use rustls_pemfile::{certs, private_key};
use std::fs::File;
use std::io::BufReader;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tracing::{error, info, warn};

pub struct Server {
    pub id: String,
    pub bind_address: String,
    pub port: u16,
    pub router: Arc<Router>,
    pub proxy_handler: Arc<ProxyHandler>,
    pub health_manager: Option<HealthCheckManager>,
    pub tls_acceptor: Option<TlsAcceptor>,
}

impl Server {
    pub fn new(config: &Config, server_config: &ServerConfig) -> Self {
        info!("Creating server: {}", server_config.id);

        let router = Arc::new(Router::new(config, server_config));
        let proxy_handler = Arc::new(ProxyHandler::new());

        let tls_acceptor = if let Some(tls_config) = &server_config.tls {
            match Self::load_tls_config(tls_config) {
                Ok(acceptor) => {
                    info!("TLS enabled for server: {}", server_config.id);
                    Some(acceptor)
                }
                Err(e) => {
                    error!(
                        "Failed to load TLS config for server {}: {}",
                        server_config.id, e
                    );
                    None
                }
            }
        } else {
            None
        };

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
            tls_acceptor,
        }
    }

    fn load_tls_config(
        tls_config: &crate::config::TlsConfig,
    ) -> Result<TlsAcceptor, Box<dyn std::error::Error + Send + Sync>> {
        let cert_file = File::open(&tls_config.cert_path)?;
        let mut cert_reader = BufReader::new(cert_file);
        let cert_chain = certs(&mut cert_reader).collect::<Result<Vec<_>, _>>()?;

        let key_file = File::open(&tls_config.key_path)?;
        let mut key_reader = BufReader::new(key_file);
        let private_key = private_key(&mut key_reader)?.ok_or("No private key found")?;

        let config = RustlsConfig::builder()
            .with_no_client_auth()
            .with_single_cert(cert_chain, private_key)?;

        Ok(TlsAcceptor::from(Arc::new(config)))
    }

    pub async fn start(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let addr = format!("{}:{}", self.bind_address, self.port);
        let protocol = if self.tls_acceptor.is_some() {
            "HTTPS"
        } else {
            "HTTP"
        };
        info!("Starting {} server {} on {}", protocol, self.id, addr);

        if let Some(manager) = self.health_manager.clone() {
            tokio::spawn(async move {
                manager.start_all().await;
            });
            info!("Health checks started for server {}", self.id);
        }

        let listener = TcpListener::bind(&addr).await?;
        info!("Server {} listening on {} ({})", self.id, addr, protocol);

        let router = self.router.clone();
        let proxy_handler = self.proxy_handler.clone();
        let server_id = self.id.clone();
        let tls_acceptor = self.tls_acceptor.clone();

        loop {
            match listener.accept().await {
                Ok((stream, remote_addr)) => {
                    let router = router.clone();
                    let proxy_handler = proxy_handler.clone();
                    let server_id = server_id.clone();
                    let tls_acceptor = tls_acceptor.clone();

                    tokio::spawn(async move {
                        let service = service_fn(move |req| {
                            let router = router.clone();
                            let proxy_handler = proxy_handler.clone();
                            let server_id = server_id.clone();

                            async move { proxy_handler.handle_request(req, router, server_id).await }
                        });

                        if let Some(acceptor) = tls_acceptor {
                            match acceptor.accept(stream).await {
                                Ok(tls_stream) => {
                                    let io = TokioIo::new(tls_stream);
                                    if let Err(err) =
                                        http1::Builder::new().serve_connection(io, service).await
                                    {
                                        error!(
                                            "Failed to serve HTTPS connection from {}: {}",
                                            remote_addr, err
                                        );
                                    }
                                }
                                Err(err) => {
                                    error!("TLS handshake failed with {}: {}", remote_addr, err);
                                }
                            }
                        } else {
                            let io = TokioIo::new(stream);
                            if let Err(err) =
                                http1::Builder::new().serve_connection(io, service).await
                            {
                                error!(
                                    "Failed to serve HTTP connection from {}: {}",
                                    remote_addr, err
                                );
                            }
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
            tls: None,
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
            tls: None,
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
