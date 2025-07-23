use crate::config::HealthCheckConfig;
use crate::target::Target;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::interval;
use tracing::{debug, info, warn};

#[derive(Clone)]
pub struct HealthChecker {
    config: HealthCheckConfig,
    targets: Vec<Arc<Target>>,
    client: reqwest::Client,
}

impl HealthChecker {
    pub fn new(config: HealthCheckConfig, targets: Vec<Arc<Target>>) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.timeout_seconds))
            .build()
            .expect("Failed to create HTTP client for health checks");

        Self {
            config,
            targets,
            client,
        }
    }

    pub async fn start(&self) {
        info!(
            "Starting health checks every {} seconds for {} targets",
            self.config.interval_seconds,
            self.targets.len()
        );

        let mut interval = interval(Duration::from_secs(self.config.interval_seconds));

        loop {
            interval.tick().await;
            self.check_all_targets().await;
        }
    }

    async fn check_all_targets(&self) {
        for target in &self.targets {
            let was_healthy = target.is_healthy();
            let is_healthy = self.check_target(target.clone()).await;

            if was_healthy != is_healthy {
                if is_healthy {
                    info!("Target {} is now healthy", target.id);
                    target.set_healthy(true);
                } else {
                    warn!("Target {} is now unhealthy", target.id);
                    target.set_healthy(false);
                }
            }
        }
    }

    async fn check_target(&self, target: Arc<Target>) -> bool {
        let path = target.health_check_path.as_deref().unwrap_or("/health");
        let url = format!("http://{}:{}{}", target.host, target.port, path);

        debug!("Health checking target {} at {}", target.id, url);

        match self.client.get(&url).send().await {
            Ok(response) => {
                let status = response.status().as_u16();
                let is_healthy = status == self.config.expected_status;

                if !is_healthy {
                    debug!(
                        "Target {} health check failed: expected status {}, got {}",
                        target.id, self.config.expected_status, status
                    );
                }

                is_healthy
            }
            Err(e) => {
                debug!("Target {} health check failed: {}", target.id, e);
                false
            }
        }
    }
}

#[derive(Clone)]
pub struct HealthCheckManager {
    checkers: Vec<HealthChecker>,
}

impl HealthCheckManager {
    pub fn new() -> Self {
        Self {
            checkers: Vec::new(),
        }
    }

    pub fn add_checker(&mut self, checker: HealthChecker) {
        self.checkers.push(checker);
    }

    pub async fn start_all(self) {
        let mut tasks = Vec::new();

        for checker in self.checkers {
            let task = tokio::spawn(async move {
                checker.start().await;
            });
            tasks.push(task);
        }

        for task in tasks {
            let _ = task.await;
        }
    }
}

impl Default for HealthCheckManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::TargetConfig;
    use tokio::time::timeout;

    fn create_test_target() -> Arc<Target> {
        let unavailable_port = 65535;
        let config = TargetConfig {
            host: "127.0.0.1".to_string(),
            port: unavailable_port,
            weight: Some(1),
            health_check_path: Some("/health".to_string()),
        };
        Arc::new(Target::new(config))
    }

    #[tokio::test]
    async fn test_health_checker_creation() {
        let config = HealthCheckConfig {
            interval_seconds: 30,
            timeout_seconds: 5,
            expected_status: 200,
        };

        let targets = vec![create_test_target()];
        let health_checker = HealthChecker::new(config, targets);

        assert_eq!(health_checker.config.interval_seconds, 30);
        assert_eq!(health_checker.targets.len(), 1);
    }

    #[tokio::test]
    async fn test_check_target_unhealthy() {
        let config = HealthCheckConfig {
            interval_seconds: 30,
            timeout_seconds: 1,
            expected_status: 200,
        };

        let target = create_test_target();
        let health_checker = HealthChecker::new(config, vec![target.clone()]);

        let result = timeout(Duration::from_secs(2), health_checker.check_target(target)).await;

        assert!(result.is_ok());
        assert!(!result.unwrap());
    }

    #[tokio::test]
    async fn test_health_check_manager() {
        let config = HealthCheckConfig {
            interval_seconds: 30,
            timeout_seconds: 1,
            expected_status: 200,
        };

        let targets = vec![create_test_target()];
        let checker = HealthChecker::new(config, targets);

        let mut manager = HealthCheckManager::new();
        manager.add_checker(checker);

        assert_eq!(manager.checkers.len(), 1);
    }

    #[tokio::test]
    async fn test_check_all_targets() {
        let config = HealthCheckConfig {
            interval_seconds: 1,
            timeout_seconds: 1,
            expected_status: 200,
        };

        let targets = vec![create_test_target(), create_test_target()];
        let health_checker = HealthChecker::new(config, targets);

        let result = timeout(Duration::from_secs(3), health_checker.check_all_targets()).await;

        assert!(result.is_ok());
    }
}
