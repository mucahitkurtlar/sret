use crate::config::{Config, ServerConfig};
use crate::target::Target;
use crate::upstream::Upstream;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, info, warn};

#[derive(Debug, Clone)]
pub struct Router {
    upstreams: HashMap<String, Arc<Upstream>>,
    routes: Vec<Route>,
}

#[derive(Debug, Clone)]
struct Route {
    domains: Vec<String>,
    paths: Vec<String>,
    upstream_id: String,
}

impl Router {
    pub fn new(config: &Config, server: &ServerConfig) -> Self {
        info!("Creating router for server: {}", server.id);

        let upstreams: HashMap<String, Arc<Upstream>> = config
            .upstreams
            .iter()
            .map(|upstream_config| {
                let upstream = Arc::new(Upstream::new(upstream_config.clone()));
                (upstream_config.id.clone(), upstream)
            })
            .collect();

        let routes: Vec<Route> = server
            .routes
            .iter()
            .map(|route_config| Route {
                domains: route_config.domains.clone().unwrap_or_default(),
                paths: route_config.paths.clone().unwrap_or_default(),
                upstream_id: route_config.upstream.clone(),
            })
            .collect();

        info!(
            "Router created with {} upstreams and {} routes",
            upstreams.len(),
            routes.len()
        );

        Self { upstreams, routes }
    }

    pub fn route_request(
        &self,
        host: Option<&str>,
        path: &str,
    ) -> Option<(Arc<Target>, Option<String>)> {
        debug!("Routing request: host={:?}, path={}", host, path);

        for route in &self.routes {
            if let Some(matched_prefix) = self.matches_route_with_prefix(route, host, path) {
                debug!(
                    "Found matching route for upstream: {}, matched prefix: {:?}",
                    route.upstream_id, matched_prefix
                );

                if let Some(upstream) = self.upstreams.get(&route.upstream_id) {
                    if let Some(target) = upstream.select_target() {
                        return Some((target, matched_prefix));
                    }
                } else {
                    warn!(
                        "Route references non-existent upstream: {}",
                        route.upstream_id
                    );
                }
            }
        }

        debug!("No matching route found for host={:?}, path={}", host, path);
        None
    }

    fn matches_route_with_prefix(
        &self,
        route: &Route,
        host: Option<&str>,
        path: &str,
    ) -> Option<Option<String>> {
        let domain_matches = if route.domains.is_empty() {
            false
        } else {
            host.is_some_and(|h| {
                let host_domain = h.split(':').next().unwrap_or(h).to_lowercase();
                route
                    .domains
                    .iter()
                    .any(|domain| domain.to_lowercase() == host_domain)
            })
        };

        let path_match = if route.paths.is_empty() {
            if domain_matches { Some(None) } else { None }
        } else {
            route.paths.iter().find_map(|route_path| {
                if path.starts_with(route_path) {
                    Some(Some(route_path.clone()))
                } else {
                    None
                }
            })
        };

        let matches = domain_matches || path_match.is_some();

        debug!(
            "Route check - Domain: {:?} (matches: {}), Path: {} (path_match: {:?}), Overall: {}, Upstream: {}",
            host, domain_matches, path, path_match, matches, route.upstream_id
        );

        if matches {
            path_match.or(Some(None))
        } else {
            None
        }
    }

    pub fn get_upstream(&self, upstream_id: &str) -> Option<Arc<Upstream>> {
        self.upstreams.get(upstream_id).cloned()
    }

    pub fn get_all_upstreams(&self) -> Vec<Arc<Upstream>> {
        self.upstreams.values().cloned().collect()
    }

    pub fn get_all_targets(&self) -> Vec<Arc<Target>> {
        self.upstreams
            .values()
            .flat_map(|upstream| upstream.all_targets().iter().cloned())
            .collect()
    }

    pub fn find_target(&self, target_id: &str) -> Option<Arc<Target>> {
        for upstream in self.upstreams.values() {
            if let Some(target) = upstream.get_target(target_id) {
                return Some(target);
            }
        }
        None
    }

    pub fn stats(&self) -> RouterStats {
        let upstream_stats = self
            .upstreams
            .values()
            .map(|upstream| upstream.stats())
            .collect();

        RouterStats {
            total_upstreams: self.upstreams.len(),
            total_routes: self.routes.len(),
            upstream_stats,
        }
    }

    pub fn has_healthy_targets(&self) -> bool {
        self.upstreams
            .values()
            .any(|upstream| upstream.has_healthy_targets())
    }
}

#[derive(Debug)]
pub struct RouterStats {
    pub total_upstreams: usize,
    pub total_routes: usize,
    pub upstream_stats: Vec<crate::upstream::UpstreamStats>,
}

impl std::fmt::Display for RouterStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Router Stats:")?;
        writeln!(f, "  Total upstreams: {}", self.total_upstreams)?;
        writeln!(f, "  Total routes: {}", self.total_routes)?;
        writeln!(f, "  Upstream details:")?;
        for upstream in &self.upstream_stats {
            writeln!(f, "    {}", upstream)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{RouteConfig, TargetConfig, UpstreamConfig};

    fn create_test_config() -> (Config, ServerConfig) {
        let config = Config {
            upstreams: vec![
                UpstreamConfig {
                    id: "backend".to_string(),
                    targets: vec![TargetConfig {
                        host: "127.0.0.1".to_string(),
                        port: 3000,
                        weight: Some(1),
                        health_check_path: None,
                    }],
                    strategy: None,
                    health_check: None,
                },
                UpstreamConfig {
                    id: "api".to_string(),
                    targets: vec![TargetConfig {
                        host: "127.0.0.1".to_string(),
                        port: 4000,
                        weight: Some(1),
                        health_check_path: None,
                    }],
                    strategy: None,
                    health_check: None,
                },
            ],
            servers: vec![],
        };

        let server = ServerConfig {
            id: "test-server".to_string(),
            bind_address: "127.0.0.1".to_string(),
            port: 8080,
            tls: None,
            routes: vec![
                RouteConfig {
                    domains: Some(vec!["api.example.com".to_string()]),
                    paths: Some(vec!["/api".to_string()]),
                    upstream: "api".to_string(),
                },
                RouteConfig {
                    domains: Some(vec!["backend.example.com".to_string()]),
                    paths: None,
                    upstream: "backend".to_string(),
                },
                RouteConfig {
                    domains: None,
                    paths: Some(vec!["/fallback".to_string()]),
                    upstream: "backend".to_string(),
                },
            ],
        };

        (config, server)
    }

    #[test]
    fn test_router_creation() {
        let (config, server) = create_test_config();
        let router = Router::new(&config, &server);

        assert_eq!(router.upstreams.len(), 2);
        assert_eq!(router.routes.len(), 3);
        assert!(router.has_healthy_targets());
    }

    #[test]
    fn test_domain_routing() {
        let (config, server) = create_test_config();
        let router = Router::new(&config, &server);

        let result = router.route_request(Some("api.example.com"), "/api/users");
        assert!(result.is_some());
        let (target, prefix) = result.unwrap();
        assert_eq!(target.port, 4000);
        assert_eq!(prefix, Some("/api".to_string()));

        let result = router.route_request(Some("backend.example.com"), "/anything");
        assert!(result.is_some());
        let (target, prefix) = result.unwrap();
        assert_eq!(target.port, 3000);
        assert_eq!(prefix, None);
    }

    #[test]
    fn test_path_routing() {
        let (config, server) = create_test_config();
        let router = Router::new(&config, &server);

        let result = router.route_request(Some("unknown.example.com"), "/fallback/test");
        assert!(result.is_some());
        let (target, prefix) = result.unwrap();
        assert_eq!(target.port, 3000);
        assert_eq!(prefix, Some("/fallback".to_string()));
    }

    #[test]
    fn test_combined_domain_and_path_routing() {
        let (config, server) = create_test_config();
        let router = Router::new(&config, &server);

        let result = router.route_request(Some("api.example.com"), "/api/test");
        assert!(result.is_some());
        let (target, prefix) = result.unwrap();
        assert_eq!(target.port, 4000);
        assert_eq!(prefix, Some("/api".to_string()));

        let result = router.route_request(Some("api.example.com"), "/other");
        assert!(result.is_some());
    }

    #[test]
    fn test_no_match_returns_none() {
        let (config, server) = create_test_config();
        let router = Router::new(&config, &server);

        let result = router.route_request(Some("unknown.com"), "/unknown");
        assert!(result.is_none());
    }

    #[test]
    fn test_router_stats() {
        let (config, server) = create_test_config();
        let router = Router::new(&config, &server);

        let stats = router.stats();
        assert_eq!(stats.total_upstreams, 2);
        assert_eq!(stats.total_routes, 3);
        assert_eq!(stats.upstream_stats.len(), 2);
    }

    #[test]
    fn test_find_target() {
        let (config, server) = create_test_config();
        let router = Router::new(&config, &server);

        let target = router.find_target("127.0.0.1:3000");
        assert!(target.is_some());
        assert_eq!(target.unwrap().id, "127.0.0.1:3000");

        let missing_target = router.find_target("nonexistent:9999");
        assert!(missing_target.is_none());
    }
}
