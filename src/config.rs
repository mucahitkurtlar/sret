use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub servers: Vec<ServerConfig>,
    pub upstreams: Vec<UpstreamConfig>,
}

impl Config {
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let config: Config = serde_yml::from_str(&content)?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        let upstream_ids: std::collections::HashSet<_> =
            self.upstreams.iter().map(|u| &u.id).collect();

        for server in &self.servers {
            for route in &server.routes {
                if !upstream_ids.contains(&route.upstream) {
                    return Err(anyhow::anyhow!(
                        "Route references non-existent upstream: {}",
                        route.upstream
                    ));
                }
            }
        }

        let mut server_ids = std::collections::HashSet::new();
        for server in &self.servers {
            if !server_ids.insert(&server.id) {
                return Err(anyhow::anyhow!("Duplicate server ID: {}", server.id));
            }
        }

        let mut upstream_ids = std::collections::HashSet::new();
        for upstream in &self.upstreams {
            if !upstream_ids.insert(&upstream.id) {
                return Err(anyhow::anyhow!("Duplicate upstream ID: {}", upstream.id));
            }
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub id: String,
    pub bind_address: String,
    pub port: u16,
    pub routes: Vec<RouteConfig>,
    pub tls: Option<TlsConfig>,
    pub cache: Option<CacheConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlsConfig {
    pub cert_path: String,
    pub key_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheConfig {
    pub max_size: usize,
    pub default_ttl_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteConfig {
    pub domains: Option<Vec<String>>,
    pub paths: Option<Vec<String>>,
    pub upstream: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpstreamConfig {
    pub id: String,
    pub targets: Vec<TargetConfig>,
    pub strategy: Option<LoadBalancerStrategy>,
    pub health_check: Option<HealthCheckConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetConfig {
    pub host: String,
    pub port: u16,
    pub weight: Option<u32>,
    pub health_check_path: Option<String>,
}

impl TargetConfig {
    pub fn weight(&self) -> u32 {
        self.weight.unwrap_or(1)
    }

    pub fn address(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    pub fn health_check_url(&self) -> String {
        let path = self.health_check_path.as_deref().unwrap_or("/health");
        format!("http://{}:{}{}", self.host, self.port, path)
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq)]
pub enum LoadBalancerStrategy {
    #[default]
    RoundRobin,
    LeastConnections,
    Random,
    WeightedRoundRobin,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthCheckConfig {
    pub interval_seconds: u64,
    pub timeout_seconds: u64,
    pub expected_status: u16,
}

impl Default for HealthCheckConfig {
    fn default() -> Self {
        Self {
            interval_seconds: 30,
            timeout_seconds: 5,
            expected_status: 200,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_config_validation_success() {
        let config = Config {
            servers: vec![ServerConfig {
                id: "test".to_string(),
                bind_address: "127.0.0.1".to_string(),
                port: 8080,
                tls: None,
                cache: None,
                routes: vec![RouteConfig {
                    domains: Some(vec!["example.com".to_string()]),
                    paths: None,
                    upstream: "backend".to_string(),
                }],
            }],
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
        };

        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_validation_missing_upstream() {
        let config = Config {
            servers: vec![ServerConfig {
                id: "test".to_string(),
                bind_address: "127.0.0.1".to_string(),
                port: 8080,
                tls: None,
                cache: None,
                routes: vec![RouteConfig {
                    domains: Some(vec!["example.com".to_string()]),
                    paths: None,
                    upstream: "nonexistent".to_string(),
                }],
            }],
            upstreams: vec![],
        };

        assert!(config.validate().is_err());
    }

    #[test]
    fn test_target_config_defaults() {
        let target = TargetConfig {
            host: "127.0.0.1".to_string(),
            port: 3000,
            weight: None,
            health_check_path: None,
        };

        assert_eq!(target.weight(), 1);
        assert_eq!(target.address(), "127.0.0.1:3000");
        assert_eq!(target.health_check_url(), "http://127.0.0.1:3000/health");
    }

    #[test]
    fn test_config_from_yaml() {
        let yaml_content = r#"
servers:
  - id: "main"
    bind_address: "0.0.0.0"
    port: 80
    routes:
      - domains: ["example.com"]
        upstream: "backend"

upstreams:
  - id: "backend"
    targets:
      - host: "127.0.0.1"
        port: 3000
"#;

        let mut temp_file = NamedTempFile::new().unwrap();
        temp_file.write_all(yaml_content.as_bytes()).unwrap();

        let config = Config::from_file(temp_file.path()).unwrap();
        assert_eq!(config.servers.len(), 1);
        assert_eq!(config.upstreams.len(), 1);
        assert_eq!(config.servers[0].id, "main");
        assert_eq!(config.upstreams[0].id, "backend");
    }
}
