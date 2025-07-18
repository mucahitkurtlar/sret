use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub proxy: ProxyConfig,
    pub load_balancer: LoadBalancerConfig,
    pub upstreams: Vec<UpstreamConfig>,
    pub routes: Option<Vec<RouteConfig>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyConfig {
    pub bind_address: String,
    pub port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoadBalancerConfig {
    pub strategy: LoadBalancerStrategy,
    pub health_check: Option<HealthCheckConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LoadBalancerStrategy {
    RoundRobin,
    LeastConnections,
    Random,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthCheckConfig {
    pub interval_seconds: u64,
    pub timeout_seconds: u64,
    pub expected_status: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpstreamConfig {
    pub id: String,
    pub host: String,
    pub port: u16,
    pub weight: u32,
    pub health_check_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteConfig {
    pub domain: String,
    pub upstream_ids: Vec<String>,
    pub load_balancer_strategy: Option<LoadBalancerStrategy>,
}

impl Config {
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let content = std::fs::read_to_string(path).context("Failed to read configuration file")?;

        let config: Config =
            serde_yml::from_str(&content).context("Failed to parse configuration file")?;

        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        if self.proxy.bind_address.is_empty() {
            anyhow::bail!("Proxy bind address cannot be empty");
        }

        if self.proxy.port == 0 {
            anyhow::bail!("Proxy port must be a valid non-zero port number");
        }

        if let Some(hc) = &self.load_balancer.health_check {
            if hc.interval_seconds == 0 {
                anyhow::bail!("Health check interval must be greater than zero");
            }
            if hc.timeout_seconds == 0 {
                anyhow::bail!("Health check timeout must be greater than zero");
            }
            if hc.expected_status < 100 || hc.expected_status > 599 {
                anyhow::bail!("Expected status code must be between 100 and 599");
            }
        }

        if self.upstreams.is_empty() {
            anyhow::bail!("At least one upstream must be configured");
        }

        for upstream in &self.upstreams {
            if upstream.host.is_empty() {
                anyhow::bail!("Upstream '{}' has empty host", upstream.id);
            }
            if upstream.port == 0 {
                anyhow::bail!("Upstream '{}' has invalid port", upstream.id);
            }
            if upstream.id.is_empty() {
                anyhow::bail!("Upstream has empty ID");
            }
            if let Some(path) = &upstream.health_check_path {
                if !path.starts_with('/') {
                    anyhow::bail!(
                        "Health check path '{}' for upstream '{}' must start with '/'",
                        path,
                        upstream.id
                    );
                }
            }
        }

        if let Some(routes) = &self.routes {
            for route in routes {
                if route.domain.is_empty() {
                    anyhow::bail!("Route has empty domain");
                }
                if route.upstream_ids.is_empty() {
                    anyhow::bail!("Route '{}' has no upstream IDs", route.domain);
                }

                for upstream_id in &route.upstream_ids {
                    if !self.upstreams.iter().any(|u| &u.id == upstream_id) {
                        anyhow::bail!(
                            "Route '{}' references non-existent upstream '{}'",
                            route.domain,
                            upstream_id
                        );
                    }
                }
            }
        }

        Ok(())
    }
}
