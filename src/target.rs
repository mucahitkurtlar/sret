use crate::config::TargetConfig;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct Target {
    pub id: String,
    pub host: String,
    pub port: u16,
    pub weight: u32,
    pub health_check_path: Option<String>,
    pub healthy: Arc<AtomicBool>,
    pub active_connections: Arc<AtomicUsize>,
    pub last_health_check: Arc<std::sync::Mutex<Option<Instant>>>,
}

impl Target {
    pub fn new(config: TargetConfig) -> Self {
        let id = format!("{}:{}", config.host, config.port);
        let host = config.host.clone();
        let health_check_path = config.health_check_path.clone();
        let weight = config.weight();

        Self {
            id,
            host,
            port: config.port,
            weight,
            health_check_path,
            healthy: Arc::new(AtomicBool::new(true)),
            active_connections: Arc::new(AtomicUsize::new(0)),
            last_health_check: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    pub fn address(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://{}:{}{}", self.host, self.port, path)
    }

    pub fn health_check_url(&self) -> String {
        let path = self.health_check_path.as_deref().unwrap_or("/health");
        self.url(path)
    }

    pub fn is_healthy(&self) -> bool {
        self.healthy.load(Ordering::Relaxed)
    }

    pub fn set_healthy(&self, healthy: bool) {
        self.healthy.store(healthy, Ordering::Relaxed);

        if let Ok(mut last_check) = self.last_health_check.lock() {
            *last_check = Some(Instant::now());
        }
    }

    pub fn active_connections(&self) -> usize {
        self.active_connections.load(Ordering::Relaxed)
    }

    pub fn increment_connections(&self) {
        self.active_connections.fetch_add(1, Ordering::Relaxed);
    }

    pub fn decrement_connections(&self) {
        self.active_connections.fetch_sub(1, Ordering::Relaxed);
    }

    pub fn effective_weight(&self) -> u32 {
        if self.is_healthy() { self.weight } else { 0 }
    }

    pub fn connection_score(&self) -> f64 {
        if !self.is_healthy() {
            return f64::INFINITY;
        }

        let connections = self.active_connections() as f64;
        let weight = self.weight as f64;

        connections / weight.max(1.0)
    }

    pub fn time_since_last_health_check(&self) -> Option<Duration> {
        self.last_health_check
            .lock()
            .ok()?
            .map(|instant| instant.elapsed())
    }
}

impl PartialEq for Target {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl std::fmt::Display for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}({}) [healthy: {}, connections: {}, weight: {}]",
            self.id,
            self.address(),
            self.is_healthy(),
            self.active_connections(),
            self.weight
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_target() -> Target {
        let config = TargetConfig {
            host: "127.0.0.1".to_string(),
            port: 3000,
            weight: Some(2),
            health_check_path: Some("/custom-health".to_string()),
        };
        Target::new(config)
    }

    #[test]
    fn test_target_creation() {
        let target = create_test_target();

        assert_eq!(target.id, "127.0.0.1:3000");
        assert_eq!(target.host, "127.0.0.1");
        assert_eq!(target.port, 3000);
        assert_eq!(target.weight, 2);
        assert!(target.is_healthy());
        assert_eq!(target.active_connections(), 0);
    }

    #[test]
    fn test_target_urls() {
        let target = create_test_target();

        assert_eq!(target.address(), "127.0.0.1:3000");
        assert_eq!(target.url("/api/test"), "http://127.0.0.1:3000/api/test");
        assert_eq!(
            target.health_check_url(),
            "http://127.0.0.1:3000/custom-health"
        );
    }

    #[test]
    fn test_health_status() {
        let target = create_test_target();

        assert!(target.is_healthy());

        target.set_healthy(false);
        assert!(!target.is_healthy());

        target.set_healthy(true);
        assert!(target.is_healthy());
    }

    #[test]
    fn test_connection_tracking() {
        let target = create_test_target();

        assert_eq!(target.active_connections(), 0);

        target.increment_connections();
        assert_eq!(target.active_connections(), 1);

        target.increment_connections();
        assert_eq!(target.active_connections(), 2);

        target.decrement_connections();
        assert_eq!(target.active_connections(), 1);
    }

    #[test]
    fn test_effective_weight() {
        let target = create_test_target();

        assert_eq!(target.effective_weight(), 2);

        target.set_healthy(false);
        assert_eq!(target.effective_weight(), 0);

        target.set_healthy(true);
        assert_eq!(target.effective_weight(), 2);
    }

    #[test]
    fn test_connection_score() {
        let target = create_test_target();

        assert_eq!(target.connection_score(), 0.0);

        target.increment_connections();
        assert_eq!(target.connection_score(), 0.5);

        target.set_healthy(false);
        assert_eq!(target.connection_score(), f64::INFINITY);
    }

    #[test]
    fn test_display() {
        let target = create_test_target();
        let display_str = format!("{}", target);

        assert!(display_str.contains("127.0.0.1:3000"));
        assert!(display_str.contains("healthy: true"));
        assert!(display_str.contains("connections: 0"));
        assert!(display_str.contains("weight: 2"));
    }
}
