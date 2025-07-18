use crate::config::{LoadBalancerStrategy, UpstreamConfig};
use rand::Rng;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::RwLock;

#[derive(Debug, Clone)]
pub struct Upstream {
    pub id: String,
    pub host: String,
    pub port: u16,
    pub weight: u32,
    pub health_check_path: Option<String>,
    pub healthy: bool,
}

impl From<UpstreamConfig> for Upstream {
    fn from(upstream: UpstreamConfig) -> Self {
        Self {
            id: upstream.id,
            host: upstream.host,
            port: upstream.port,
            weight: upstream.weight,
            health_check_path: upstream.health_check_path,
            healthy: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct LoadBalancer {
    strategy: LoadBalancerStrategy,
    upstreams: Arc<RwLock<Vec<Upstream>>>,
    round_robin_counter: Arc<AtomicUsize>,
    connection_counts: Arc<RwLock<HashMap<String, usize>>>,
}

impl LoadBalancer {
    pub fn new(strategy: LoadBalancerStrategy, upstreams: Vec<Upstream>) -> Self {
        let connection_counts = upstreams.iter().map(|u| (u.id.clone(), 0)).collect();

        Self {
            strategy,
            upstreams: Arc::new(RwLock::new(upstreams)),
            round_robin_counter: Arc::new(AtomicUsize::new(0)),
            connection_counts: Arc::new(RwLock::new(connection_counts)),
        }
    }

    pub async fn select_upstream(&self) -> Option<Upstream> {
        let upstreams = self.upstreams.read().await;
        let available_upstreams: Vec<&Upstream> = upstreams.iter().filter(|u| u.healthy).collect();

        if available_upstreams.is_empty() {
            return None;
        }

        match self.strategy {
            LoadBalancerStrategy::RoundRobin => self.select_round_robin(&available_upstreams).await,
            LoadBalancerStrategy::LeastConnections => {
                self.select_least_connections(&available_upstreams).await
            }
            LoadBalancerStrategy::Random => self.select_random(&available_upstreams).await,
        }
    }

    async fn select_round_robin(&self, upstreams: &[&Upstream]) -> Option<Upstream> {
        let index = self.round_robin_counter.fetch_add(1, Ordering::Relaxed) % upstreams.len();
        upstreams.get(index).map(|&u| u.clone())
    }

    async fn select_least_connections(&self, upstreams: &[&Upstream]) -> Option<Upstream> {
        let connection_counts = self.connection_counts.read().await;

        let mut min_connections = usize::MAX;
        let mut selected_upstream = None;

        for upstream in upstreams {
            let connections = connection_counts.get(&upstream.id).copied().unwrap_or(0);
            if connections < min_connections {
                min_connections = connections;
                selected_upstream = Some(upstream);
            }
        }

        selected_upstream.map(|&u| u.clone())
    }

    async fn select_random(&self, upstreams: &[&Upstream]) -> Option<Upstream> {
        let mut rng = rand::rng();
        let index = rng.random_range(0..upstreams.len());
        upstreams.get(index).map(|&u| u.clone())
    }

    pub async fn increment_connections(&self, upstream_id: &str) {
        let mut connection_counts = self.connection_counts.write().await;
        *connection_counts
            .entry(upstream_id.to_string())
            .or_insert(0) += 1;
    }

    pub async fn decrement_connections(&self, upstream_id: &str) {
        let mut connection_counts = self.connection_counts.write().await;
        if let Some(count) = connection_counts.get_mut(upstream_id) {
            if *count > 0 {
                *count -= 1;
            }
        }
    }

    pub async fn update_upstreams(&self, upstreams: Vec<Upstream>) {
        let mut current_upstreams = self.upstreams.write().await;
        *current_upstreams = upstreams;

        let mut connection_counts = self.connection_counts.write().await;
        for upstream in current_upstreams.iter() {
            connection_counts.entry(upstream.id.clone()).or_insert(0);
        }
    }

    pub async fn set_upstream_status(&self, upstream_id: &str, enabled: bool) {
        let mut upstreams = self.upstreams.write().await;
        for upstream in upstreams.iter_mut() {
            if upstream.id == upstream_id {
                upstream.healthy = enabled;
                break;
            }
        }
    }

    pub async fn get_upstreams(&self) -> Vec<Upstream> {
        self.upstreams.read().await.clone()
    }
}
