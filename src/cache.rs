use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper::header::{AUTHORIZATION, CACHE_CONTROL, COOKIE, ETAG, EXPIRES, HOST, LAST_MODIFIED};
use hyper::{HeaderMap, Response, StatusCode};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use tracing::{debug, warn};

pub struct CacheControl {
    pub max_age: Option<u64>,
    pub no_cache: bool,
    pub no_store: bool,
}

impl CacheControl {
    pub fn from_headers(headers: &HeaderMap) -> Self {
        let mut max_age = None;
        let mut no_cache = false;
        let mut no_store = false;

        if let Some(cache_control) = headers.get(CACHE_CONTROL) {
            if let Ok(value) = cache_control.to_str() {
                for directive in value.split(',') {
                    let directive = directive.trim();
                    if let Some(stripped) = directive.strip_prefix("max-age=") {
                        if let Ok(age) = stripped.parse::<u64>() {
                            max_age = Some(age);
                        }
                    } else if directive == "no-cache" {
                        no_cache = true;
                    } else if directive == "no-store" {
                        no_store = true;
                    }
                }
            }
        }

        Self {
            max_age,
            no_cache,
            no_store,
        }
    }
}

impl From<&HeaderMap> for CacheControl {
    fn from(headers: &HeaderMap) -> Self {
        Self::from_headers(headers)
    }
}

#[derive(Clone)]
pub struct CacheEntry {
    pub response_body: Vec<u8>,
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub expires_at: Instant,
    pub created_at: Instant,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub last_accessed: Instant,
}

#[derive(Debug, Default)]
pub struct CacheMetrics {
    pub hits: AtomicU64,
    pub misses: AtomicU64,
    pub evictions: AtomicU64,
    pub size: AtomicUsize,
}

impl CacheMetrics {
    pub fn hit_rate(&self) -> f64 {
        let hits = self.hits.load(Ordering::Relaxed) as f64;
        let total = hits + self.misses.load(Ordering::Relaxed) as f64;
        if total > 0.0 { hits / total } else { 0.0 }
    }

    pub fn record_hit(&self) {
        self.hits.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_miss(&self) {
        self.misses.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_eviction(&self) {
        self.evictions.fetch_add(1, Ordering::Relaxed);
    }

    pub fn update_size(&self, size: usize) {
        self.size.store(size, Ordering::Relaxed);
    }
}

pub struct ResponseCache {
    entries: Arc<RwLock<HashMap<String, CacheEntry>>>,
    max_size: usize,
    default_ttl: Duration,
    metrics: Arc<CacheMetrics>,
}

impl ResponseCache {
    pub fn new(max_size: usize, default_ttl: Duration) -> Self {
        Self {
            entries: Arc::new(RwLock::new(HashMap::new())),
            max_size,
            default_ttl,
            metrics: Arc::new(CacheMetrics::default()),
        }
    }

    pub fn get(&self, key: &str, headers: &HeaderMap) -> Option<Response<Full<Bytes>>> {
        let entries = self.entries.read().unwrap();
        let cache_control = CacheControl::from(headers);

        if let Some(entry) = entries.get(key) {
            if entry.expires_at > Instant::now()
                && !cache_control.no_cache
                && !cache_control.no_store
                && (cache_control.max_age.is_none()
                    || entry.created_at.elapsed().as_secs() < cache_control.max_age.unwrap())
            {
                debug!("Cache hit for key: {}", key);
                self.metrics.record_hit();

                let mut response = Response::builder().status(entry.status);

                for (name, value) in &entry.headers {
                    response = response.header(name, value);
                }

                response = response.header("x-cache", "hit");
                response = response.header(
                    "x-cache-age",
                    entry.created_at.elapsed().as_secs().to_string(),
                );

                return Some(
                    response
                        .body(Full::new(Bytes::from(entry.response_body.clone())))
                        .unwrap(),
                );
            } else {
                debug!("Cache entry expired or not cacheable for key: {}", key);
            }
        }

        debug!("Cache miss for key: {}", key);
        self.metrics.record_miss();
        None
    }

    pub async fn put(
        &self,
        key: String,
        response: Response<Full<Bytes>>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let status = response.status();
        let headers = response.headers().clone();

        if !self.is_cacheable(&status, &headers) {
            debug!("Response not cacheable for key: {}", key);
            return Ok(());
        }

        let body_bytes = match response.into_body().collect().await {
            Ok(collected) => collected.to_bytes().to_vec(),
            Err(e) => {
                warn!("Failed to read response body for caching: {}", e);
                return Err(Box::new(e));
            }
        };

        let ttl = self.calculate_ttl(&headers);
        let now = Instant::now();

        let entry = CacheEntry {
            response_body: body_bytes,
            status,
            headers: headers.clone(),
            expires_at: now + ttl,
            created_at: now,
            etag: headers
                .get(ETAG)
                .and_then(|v| v.to_str().ok().map(String::from)),
            last_modified: headers
                .get(LAST_MODIFIED)
                .and_then(|v| v.to_str().ok().map(String::from)),
            last_accessed: now,
        };

        let mut entries = self.entries.write().unwrap();

        if entries.len() >= self.max_size {
            self.evict_lru(&mut entries);
        }

        entries.insert(key.clone(), entry);
        self.metrics.update_size(entries.len());

        debug!("Response cached for key: {} (TTL: {:?})", key, ttl);
        Ok(())
    }

    pub fn metrics(&self) -> Arc<CacheMetrics> {
        self.metrics.clone()
    }

    fn is_cacheable(&self, status: &StatusCode, headers: &HeaderMap) -> bool {
        let cache_control = CacheControl::from(headers);
        if cache_control.no_cache || cache_control.no_store {
            return false;
        }

        matches!(
            *status,
            StatusCode::OK
                | StatusCode::NON_AUTHORITATIVE_INFORMATION
                | StatusCode::NO_CONTENT
                | StatusCode::PARTIAL_CONTENT
                | StatusCode::MULTIPLE_CHOICES
                | StatusCode::MOVED_PERMANENTLY
                | StatusCode::NOT_FOUND
                | StatusCode::METHOD_NOT_ALLOWED
                | StatusCode::GONE
                | StatusCode::URI_TOO_LONG
        )
    }

    fn calculate_ttl(&self, headers: &HeaderMap) -> Duration {
        if let Some(max_age) = CacheControl::from(headers).max_age {
            return Duration::from_secs(max_age);
        }

        if let Some(expires) = headers.get(EXPIRES) {
            if let Ok(expires_str) = expires.to_str() {
                if let Some(expires_at) = parse_expires_header(expires_str) {
                    return expires_at.duration_since(Instant::now());
                }
            }
        }

        self.default_ttl
    }

    fn evict_lru(&self, entries: &mut HashMap<String, CacheEntry>) {
        if entries.is_empty() {
            return;
        }

        let oldest_key = entries
            .iter()
            .min_by_key(|(_, entry)| entry.last_accessed)
            .map(|(key, _)| key.clone());

        if let Some(key) = oldest_key {
            entries.remove(&key);
            self.metrics.record_eviction();
            debug!("Evicted cache entry: {}", key);
        }
    }
}

pub fn generate_cache_key(
    method: &str,
    path: &str,
    headers: &HeaderMap,
    query: Option<&str>,
) -> String {
    let host = headers.get(HOST).and_then(|h| h.to_str().ok());
    let cookie_header = headers.get(COOKIE).and_then(|v| v.to_str().ok());
    let auth_header = headers.get(AUTHORIZATION).and_then(|v| v.to_str().ok());

    let mut key = match host {
        Some(host) => format!("{}:{}{}", method, host, path),
        None => format!("{}:{}", method, path),
    };

    if let Some(query) = query {
        key.push_str(&format!("?{}", query));
    }

    if let Some(cookie) = cookie_header {
        key.push_str(&format!(";cookie:{}", cookie));
    }

    if let Some(auth) = auth_header {
        key.push_str(&format!(";auth:{}", auth));
    }

    key
}

pub fn is_cacheable_method(method: &hyper::Method) -> bool {
    matches!(method, &hyper::Method::GET | &hyper::Method::HEAD)
}

pub fn parse_expires_header(value: &str) -> Option<Instant> {
    if let Ok(date) = httpdate::parse_http_date(value) {
        if let Ok(duration) = date.duration_since(std::time::SystemTime::now()) {
            Some(Instant::now() + duration)
        } else {
            None
        }
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hyper::{HeaderMap, StatusCode};

    #[test]
    fn test_cache_key_generation() {
        let headers = HeaderMap::new();
        assert_eq!(
            generate_cache_key("GET", "/api/users", &headers, None),
            "GET:/api/users"
        );

        let mut headers = HeaderMap::new();
        headers.insert(HOST, "example.com".parse().unwrap());
        headers.insert(COOKIE, "session_id=abc123".parse().unwrap());
        assert_eq!(
            generate_cache_key("GET", "/api/users", &headers, Some("page=1")),
            "GET:example.com/api/users?page=1;cookie:session_id=abc123"
        );
    }

    #[test]
    fn test_is_cacheable() {
        let cache = ResponseCache::new(100, Duration::from_secs(300));
        let mut headers = HeaderMap::new();

        assert!(cache.is_cacheable(&StatusCode::OK, &headers));

        headers.insert("cache-control", "no-cache".parse().unwrap());
        assert!(!cache.is_cacheable(&StatusCode::OK, &headers));

        headers.insert("cache-control", "no-store".parse().unwrap());
        assert!(!cache.is_cacheable(&StatusCode::OK, &headers));
    }

    #[test]
    fn test_metrics() {
        let metrics = CacheMetrics::default();

        assert_eq!(metrics.hit_rate(), 0.0);

        metrics.record_hit();
        metrics.record_miss();

        assert_eq!(metrics.hit_rate(), 0.5);
        assert_eq!(metrics.hits.load(Ordering::Relaxed), 1);
        assert_eq!(metrics.misses.load(Ordering::Relaxed), 1);
    }
}
