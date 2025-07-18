#[allow(unused_imports)]
use crate::config::{Config, LoadBalancerStrategy, RouteConfig, UpstreamConfig};
use crate::load_balancer::{LoadBalancer, Upstream};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::debug;

#[derive(Debug)]
pub struct DomainRouter {
    routes: HashMap<String, RouteInfo>,
    default_load_balancer: Arc<LoadBalancer>,
    upstreams_map: Arc<RwLock<HashMap<String, Upstream>>>,
}

#[derive(Debug)]
struct RouteInfo {
    load_balancer: Arc<LoadBalancer>,
    upstream_ids: Vec<String>,
}

impl DomainRouter {
    pub fn new(config: &Config) -> Self {
        let upstreams_map: HashMap<String, Upstream> = config
            .upstreams
            .iter()
            .map(|u| (u.id.clone(), Upstream::from(u.clone())))
            .collect();

        let default_load_balancer = Arc::new(LoadBalancer::new(
            config.load_balancer.strategy.clone(),
            config
                .upstreams
                .iter()
                .cloned()
                .map(Upstream::from)
                .collect(),
        ));

        let mut routes = HashMap::new();

        if let Some(route_configs) = &config.routes {
            for route_config in route_configs {
                let route_upstreams: Vec<Upstream> = route_config
                    .upstream_ids
                    .iter()
                    .filter_map(|id| upstreams_map.get(id))
                    .cloned()
                    .collect();

                if !route_upstreams.is_empty() {
                    let strategy = route_config
                        .load_balancer_strategy
                        .clone()
                        .unwrap_or_else(|| config.load_balancer.strategy.clone());

                    let route_load_balancer =
                        Arc::new(LoadBalancer::new(strategy, route_upstreams));

                    routes.insert(
                        route_config.domain.clone(),
                        RouteInfo {
                            load_balancer: route_load_balancer,
                            upstream_ids: route_config.upstream_ids.clone(),
                        },
                    );
                }
            }
        }

        Self {
            routes,
            default_load_balancer,
            upstreams_map: Arc::new(RwLock::new(upstreams_map)),
        }
    }

    pub async fn route_request(
        &self,
        host: Option<&str>,
    ) -> Option<(Arc<LoadBalancer>, Option<Upstream>)> {
        let domain = host.map(|h| h.split(':').next().unwrap_or(h).to_lowercase());

        debug!("Routing request for domain: {:?}", domain);

        if let Some(ref domain_str) = domain {
            if let Some(route_info) = self.routes.get(domain_str) {
                debug!("Found route for domain: {}", domain_str);
                let upstream = route_info.load_balancer.select_upstream().await;
                return Some((route_info.load_balancer.clone(), upstream));
            }
        }

        debug!("Using default load balancer for domain: {:?}", domain);
        let upstream = self.default_load_balancer.select_upstream().await;
        Some((self.default_load_balancer.clone(), upstream))
    }

    pub async fn increment_connections(&self, upstream_id: &str, host: Option<&str>) {
        let domain = host.map(|h| h.split(':').next().unwrap_or(h).to_lowercase());

        if let Some(domain) = domain {
            if let Some(route_info) = self.routes.get(&domain) {
                route_info
                    .load_balancer
                    .increment_connections(upstream_id)
                    .await;
                return;
            }
        }

        self.default_load_balancer
            .increment_connections(upstream_id)
            .await;
    }

    pub async fn decrement_connections(&self, upstream_id: &str, host: Option<&str>) {
        let domain = host.map(|h| h.split(':').next().unwrap_or(h).to_lowercase());

        if let Some(domain) = domain {
            if let Some(route_info) = self.routes.get(&domain) {
                route_info
                    .load_balancer
                    .decrement_connections(upstream_id)
                    .await;
                return;
            }
        }

        self.default_load_balancer
            .decrement_connections(upstream_id)
            .await;
    }

    pub async fn update_upstream_status(&self, upstream_id: &str, enabled: bool) {
        self.default_load_balancer
            .set_upstream_status(upstream_id, enabled)
            .await;

        for route_info in self.routes.values() {
            if route_info.upstream_ids.contains(&upstream_id.to_string()) {
                route_info
                    .load_balancer
                    .set_upstream_status(upstream_id, enabled)
                    .await;
            }
        }

        let mut upstreams_map = self.upstreams_map.write().await;
        if let Some(upstream) = upstreams_map.get_mut(upstream_id) {
            upstream.healthy = enabled;
        }
    }

    pub async fn get_all_upstreams(&self) -> Vec<Upstream> {
        let upstreams_map = self.upstreams_map.read().await;
        upstreams_map.values().cloned().collect()
    }

    pub fn get_routes(&self) -> Vec<String> {
        self.routes.keys().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{HealthCheckConfig, LoadBalancerConfig, ProxyConfig};

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
            upstreams: vec![
                UpstreamConfig {
                    id: "service1".to_string(),
                    host: "127.0.0.1".to_string(),
                    port: 3000,
                    weight: 1,
                    health_check_path: Some("/health".to_string()),
                },
                UpstreamConfig {
                    id: "service2".to_string(),
                    host: "127.0.0.1".to_string(),
                    port: 3001,
                    weight: 1,
                    health_check_path: Some("/health".to_string()),
                },
                UpstreamConfig {
                    id: "service3".to_string(),
                    host: "127.0.0.1".to_string(),
                    port: 3002,
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
                    load_balancer_strategy: Some(LoadBalancerStrategy::Random),
                },
            ]),
        }
    }

    #[tokio::test]
    async fn test_domain_routing() {
        let config = create_test_config();
        let router = DomainRouter::new(&config);

        let (_, upstream) = router
            .route_request(Some("service-1.home.arpa"))
            .await
            .unwrap();
        assert!(upstream.is_some());
        assert_eq!(upstream.unwrap().id, "service1");

        let (_, upstream) = router
            .route_request(Some("service-2.home.arpa"))
            .await
            .unwrap();
        assert!(upstream.is_some());
        assert_eq!(upstream.unwrap().id, "service2");
    }

    #[tokio::test]
    async fn test_default_routing() {
        let config = create_test_config();
        let router = DomainRouter::new(&config);

        let (_, upstream) = router
            .route_request(Some("unknown.domain.com"))
            .await
            .unwrap();
        assert!(upstream.is_some());

        let upstream = upstream.unwrap();
        assert!(["service1", "service2", "service3"].contains(&upstream.id.as_str()));
    }

    #[tokio::test]
    async fn test_no_host_header() {
        let config = create_test_config();
        let router = DomainRouter::new(&config);

        let (_, upstream) = router.route_request(None).await.unwrap();
        assert!(upstream.is_some());
    }

    #[tokio::test]
    async fn test_upstream_status_update() {
        let config = create_test_config();
        let router = DomainRouter::new(&config);

        router.update_upstream_status("service1", false).await;

        let (_, upstream) = router
            .route_request(Some("service-1.home.arpa"))
            .await
            .unwrap();
        assert!(upstream.is_none());
    }

    #[tokio::test]
    async fn test_connection_tracking() {
        let config = create_test_config();
        let router = DomainRouter::new(&config);

        router
            .increment_connections("service1", Some("service-1.home.arpa"))
            .await;
        router
            .decrement_connections("service1", Some("service-1.home.arpa"))
            .await;
    }
}
