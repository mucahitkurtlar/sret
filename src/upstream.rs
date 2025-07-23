use crate::config::{LoadBalancerStrategy, UpstreamConfig};
use crate::target::Target;
use rand::prelude::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tracing::warn;

#[derive(Debug, Clone)]
pub struct Upstream {
    pub id: String,
    pub targets: Vec<Arc<Target>>,
    pub strategy: LoadBalancerStrategy,
    pub round_robin_counter: Arc<AtomicUsize>,
}

impl Upstream {
    pub fn new(config: UpstreamConfig) -> Self {
        let targets = config
            .targets
            .into_iter()
            .map(|target_config| Arc::new(Target::new(target_config)))
            .collect();

        let strategy = config.strategy.unwrap_or_default();

        Self {
            id: config.id,
            targets,
            strategy,
            round_robin_counter: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub fn select_target(&self) -> Option<Arc<Target>> {
        let healthy_targets: Vec<&Arc<Target>> = self
            .targets
            .iter()
            .filter(|target| target.is_healthy())
            .collect();

        if healthy_targets.is_empty() {
            warn!("No healthy targets available for upstream: {}", self.id);
            return None;
        }

        match self.strategy {
            LoadBalancerStrategy::RoundRobin => self.select_round_robin(&healthy_targets),
            LoadBalancerStrategy::LeastConnections => {
                self.select_least_connections(&healthy_targets)
            }
            LoadBalancerStrategy::Random => self.select_random(&healthy_targets),
            LoadBalancerStrategy::WeightedRoundRobin => self.select_weighted_round_robin(),
        }
    }

    fn select_round_robin(&self, targets: &[&Arc<Target>]) -> Option<Arc<Target>> {
        if targets.is_empty() {
            return None;
        }

        let index = self.round_robin_counter.fetch_add(1, Ordering::Relaxed) % targets.len();
        Some(targets[index].clone())
    }

    fn select_least_connections(&self, targets: &[&Arc<Target>]) -> Option<Arc<Target>> {
        targets
            .iter()
            .min_by(|a, b| {
                a.connection_score()
                    .partial_cmp(&b.connection_score())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|&target| target.clone())
    }

    fn select_random(&self, targets: &[&Arc<Target>]) -> Option<Arc<Target>> {
        let mut rng = rand::rng();
        targets.choose(&mut rng).map(|&target| target.clone())
    }

    fn select_weighted_round_robin(&self) -> Option<Arc<Target>> {
        let mut weighted_targets = Vec::new();

        for target in &self.targets {
            if target.is_healthy() {
                for _ in 0..target.effective_weight() {
                    weighted_targets.push(target);
                }
            }
        }

        if weighted_targets.is_empty() {
            return None;
        }

        let index =
            self.round_robin_counter.fetch_add(1, Ordering::Relaxed) % weighted_targets.len();
        Some(weighted_targets[index].clone())
    }

    pub fn all_targets(&self) -> &[Arc<Target>] {
        &self.targets
    }

    pub fn healthy_targets(&self) -> Vec<Arc<Target>> {
        self.targets
            .iter()
            .filter(|target| target.is_healthy())
            .cloned()
            .collect()
    }

    pub fn healthy_target_count(&self) -> usize {
        self.targets
            .iter()
            .filter(|target| target.is_healthy())
            .count()
    }

    pub fn total_target_count(&self) -> usize {
        self.targets.len()
    }

    pub fn get_target(&self, target_id: &str) -> Option<Arc<Target>> {
        self.targets
            .iter()
            .find(|target| target.id == target_id)
            .cloned()
    }

    pub fn has_healthy_targets(&self) -> bool {
        self.targets.iter().any(|target| target.is_healthy())
    }

    pub fn stats(&self) -> UpstreamStats {
        let total_targets = self.targets.len();
        let healthy_targets = self.healthy_target_count();
        let total_connections: usize = self
            .targets
            .iter()
            .map(|target| target.active_connections())
            .sum();

        UpstreamStats {
            id: self.id.clone(),
            strategy: self.strategy.clone(),
            total_targets,
            healthy_targets,
            total_connections,
        }
    }
}

#[derive(Debug, Clone)]
pub struct UpstreamStats {
    pub id: String,
    pub strategy: LoadBalancerStrategy,
    pub total_targets: usize,
    pub healthy_targets: usize,
    pub total_connections: usize,
}

impl std::fmt::Display for UpstreamStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Upstream {} ({:?}): {}/{} healthy targets, {} total connections",
            self.id,
            self.strategy,
            self.healthy_targets,
            self.total_targets,
            self.total_connections
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::TargetConfig;

    fn create_test_upstream() -> Upstream {
        let config = UpstreamConfig {
            id: "test-upstream".to_string(),
            targets: vec![
                TargetConfig {
                    host: "127.0.0.1".to_string(),
                    port: 3000,
                    weight: Some(1),
                    health_check_path: None,
                },
                TargetConfig {
                    host: "127.0.0.1".to_string(),
                    port: 3001,
                    weight: Some(2),
                    health_check_path: None,
                },
            ],
            strategy: Some(LoadBalancerStrategy::RoundRobin),
            health_check: None,
        };
        Upstream::new(config)
    }

    #[test]
    fn test_upstream_creation() {
        let upstream = create_test_upstream();

        assert_eq!(upstream.id, "test-upstream");
        assert_eq!(upstream.targets.len(), 2);
        assert_eq!(upstream.strategy, LoadBalancerStrategy::RoundRobin);
        assert_eq!(upstream.healthy_target_count(), 2);
    }

    #[test]
    fn test_round_robin_selection() {
        let upstream = create_test_upstream();

        let target1 = upstream.select_target().unwrap();
        let target2 = upstream.select_target().unwrap();

        assert_ne!(target1.id, target2.id);
    }

    #[test]
    fn test_least_connections_selection() {
        let mut upstream = create_test_upstream();
        upstream.strategy = LoadBalancerStrategy::LeastConnections;

        let target1 = upstream.select_target().unwrap();
        target1.increment_connections();

        let target2 = upstream.select_target().unwrap();

        assert_ne!(target1.id, target2.id);
    }

    #[test]
    fn test_weighted_round_robin() {
        let mut upstream = create_test_upstream();
        upstream.strategy = LoadBalancerStrategy::WeightedRoundRobin;

        let mut target_counts = std::collections::HashMap::new();

        for _ in 0..30 {
            if let Some(target) = upstream.select_target() {
                *target_counts.entry(target.id.clone()).or_insert(0) += 1;
            }
        }

        let target1_count = target_counts.get("127.0.0.1:3000").unwrap_or(&0);
        let target2_count = target_counts.get("127.0.0.1:3001").unwrap_or(&0);

        assert!(target2_count > target1_count);
    }

    #[test]
    fn test_no_healthy_targets() {
        let upstream = create_test_upstream();

        for target in &upstream.targets {
            target.set_healthy(false);
        }

        assert!(!upstream.has_healthy_targets());
        assert_eq!(upstream.healthy_target_count(), 0);
        assert!(upstream.select_target().is_none());
    }

    #[test]
    fn test_upstream_stats() {
        let upstream = create_test_upstream();

        upstream.targets[0].increment_connections();
        upstream.targets[0].increment_connections();
        upstream.targets[1].increment_connections();

        let stats = upstream.stats();

        assert_eq!(stats.id, "test-upstream");
        assert_eq!(stats.total_targets, 2);
        assert_eq!(stats.healthy_targets, 2);
        assert_eq!(stats.total_connections, 3);
    }

    #[test]
    fn test_get_target_by_id() {
        let upstream = create_test_upstream();

        let target = upstream.get_target("127.0.0.1:3000");
        assert!(target.is_some());
        assert_eq!(target.unwrap().id, "127.0.0.1:3000");

        let missing_target = upstream.get_target("nonexistent:9999");
        assert!(missing_target.is_none());
    }
}
