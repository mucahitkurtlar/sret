use crate::config::HealthCheckConfig;
use crate::load_balancer::Upstream;
use crate::router::DomainRouter;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::interval;
use tracing::{debug, info, warn};

pub struct HealthChecker {
    config: HealthCheckConfig,
    router: Arc<DomainRouter>,
    client: reqwest::Client,
}

impl HealthChecker {
    pub fn new(config: HealthCheckConfig, router: Arc<DomainRouter>) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.timeout_seconds))
            .build()
            .expect("Failed to create HTTP client for health checks");

        Self {
            config,
            router,
            client,
        }
    }

    pub async fn start(&self) {
        info!(
            "Starting health checks every {} seconds",
            self.config.interval_seconds
        );

        let mut interval = interval(Duration::from_secs(self.config.interval_seconds));

        loop {
            interval.tick().await;
            self.check_all_upstreams().await;
        }
    }

    async fn check_all_upstreams(&self) {
        let upstreams = self.router.get_all_upstreams().await;

        for upstream in upstreams {
            let was_healthy = upstream.healthy;
            let is_healthy = self.check_upstream(&upstream).await;

            if was_healthy != is_healthy {
                if is_healthy {
                    info!("Upstream {} is now healthy", upstream.id);
                } else {
                    warn!("Upstream {} is now unhealthy", upstream.id);
                }
                self.router
                    .update_upstream_status(&upstream.id, is_healthy)
                    .await;
            }
        }
    }

    async fn check_upstream(&self, upstream: &Upstream) -> bool {
        let path = match &upstream.health_check_path {
            Some(p) => p,
            None => return true,
        };

        let url = format!("http://{}:{}{}", upstream.host, upstream.port, path);

        debug!("Health checking upstream {} at {}", upstream.id, url);

        match self.client.get(&url).send().await {
            Ok(response) => {
                let status = response.status().as_u16();
                let is_healthy = status == self.config.expected_status;

                if !is_healthy {
                    debug!(
                        "Upstream {} health check failed: expected status {}, got {}",
                        upstream.id, self.config.expected_status, status
                    );
                }

                is_healthy
            }
            Err(e) => {
                debug!("Upstream {} health check failed: {}", upstream.id, e);
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::UpstreamConfig;
    use crate::config::{Config, LoadBalancerConfig, LoadBalancerStrategy, ProxyConfig};
    use tokio::time::timeout;

    fn create_test_config() -> Config {
        Config {
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
                id: "test1".to_string(),
                host: "127.0.0.1".to_string(),
                port: 8080,
                weight: 1,
                health_check_path: Some("/health".to_string()),
            }],
            routes: None,
        }
    }

    #[tokio::test]
    async fn test_health_checker_creation() {
        let config = HealthCheckConfig {
            interval_seconds: 30,
            timeout_seconds: 5,
            expected_status: 200,
        };

        let test_config = create_test_config();
        let router = Arc::new(DomainRouter::new(&test_config));

        let health_checker = HealthChecker::new(config, router);

        assert_eq!(health_checker.config.interval_seconds, 30);
    }

    #[tokio::test]
    async fn test_check_upstream_unhealthy() {
        let config = HealthCheckConfig {
            interval_seconds: 30,
            timeout_seconds: 1,
            expected_status: 200,
        };

        let test_config = create_test_config();
        let router = Arc::new(DomainRouter::new(&test_config));

        let health_checker = HealthChecker::new(config, router);

        let upstream = Upstream {
            id: "test1".to_string(),
            host: "127.0.0.1".to_string(),
            port: 65535,
            weight: 1,
            healthy: true,
            health_check_path: Some("/health".to_string()),
        };

        let result = timeout(
            Duration::from_secs(2),
            health_checker.check_upstream(&upstream),
        )
        .await;

        assert!(result.is_ok());
        assert!(!result.unwrap());
    }
}
