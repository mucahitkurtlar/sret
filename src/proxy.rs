use crate::cache::{ResponseCache, generate_cache_key, is_cacheable_method};
use crate::router::Router;
use http_body_util::{BodyExt, Full};
use hyper::body::{Bytes, Incoming};
use hyper::{Request, Response, StatusCode};
use std::convert::Infallible;
use std::sync::Arc;
use tracing::{debug, warn};

#[derive(Debug, Clone)]
pub struct ProxyHandler {
    client: reqwest::Client,
}

impl ProxyHandler {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("Failed to create HTTP client");

        Self { client }
    }

    pub async fn handle_request(
        &self,
        req: Request<Incoming>,
        router: Arc<Router>,
        _server_id: String,
        cache: Option<Arc<ResponseCache>>,
    ) -> Result<Response<Full<Bytes>>, Infallible> {
        debug!("Handling {} request to {}", req.method(), req.uri().path());

        let cache_key = if cache.is_some() {
            if is_cacheable_method(req.method()) {
                let path = req.uri().path();
                let query = req.uri().query();
                Some(generate_cache_key(
                    req.method().as_str(),
                    path,
                    req.headers(),
                    query,
                ))
            } else {
                None
            }
        } else {
            None
        };

        if let (Some(cache), Some(key)) = (&cache, &cache_key) {
            if let Some(cached_response) = cache.get(key, req.headers()) {
                debug!("Serving cached response for {}", key);
                return Ok(cached_response);
            }
        }

        let host = req.headers().get("host").and_then(|h| h.to_str().ok());
        let path = req.uri().path();

        let (target, matched_prefix) = match router.route_request(host, path) {
            Some(result) => result,
            None => {
                warn!(
                    "No matching route found for host: {:?}, path: {}",
                    host, path
                );
                return Ok(Response::builder()
                    .status(StatusCode::NOT_FOUND)
                    .body(Full::new(Bytes::from("No matching route found")))
                    .unwrap());
            }
        };

        debug!(
            "Routing request to target: {}, matched prefix: {:?}",
            target.id, matched_prefix
        );

        let target_path = if let Some(prefix) = matched_prefix {
            path.strip_prefix(&prefix).unwrap_or(path)
        } else {
            path
        };

        let target_url = format!(
            "http://{}:{}{}{}",
            target.host,
            target.port,
            target_path,
            req.uri()
                .query()
                .map(|q| format!("?{}", q))
                .unwrap_or_default()
        );

        debug!("Proxying to: {}", target_url);

        match self
            .do_proxy_request(req, &target_url, cache.as_ref(), cache_key.as_ref())
            .await
        {
            Ok(response) => Ok(response),
            Err(e) => {
                warn!("Proxy request failed: {}", e);
                Ok(Response::builder()
                    .status(StatusCode::BAD_GATEWAY)
                    .body(Full::new(Bytes::from("Bad Gateway")))
                    .unwrap())
            }
        }
    }

    async fn do_proxy_request(
        &self,
        req: Request<Incoming>,
        target_url: &str,
        cache: Option<&Arc<ResponseCache>>,
        cache_key: Option<&String>,
    ) -> Result<Response<Full<Bytes>>, Box<dyn std::error::Error + Send + Sync>> {
        let method = req.method().clone();
        let headers = req.headers().clone();

        let body_bytes = match req.into_body().collect().await {
            Ok(collected) => collected.to_bytes(),
            Err(e) => return Err(Box::new(e)),
        };

        let mut request_builder = self.client.request(method, target_url);

        for (name, value) in headers.iter() {
            if name != "host" {
                if let Ok(value_str) = value.to_str() {
                    request_builder = request_builder.header(name, value_str);
                }
            }
        }

        if !body_bytes.is_empty() {
            request_builder = request_builder.body(body_bytes.to_vec());
        }

        let response = request_builder.send().await?;
        let status = response.status();
        let response_headers = response.headers().clone();
        let response_body = response.bytes().await?;

        let mut response_builder = Response::builder().status(status);

        for (name, value) in response_headers.iter() {
            response_builder = response_builder.header(name, value);
        }

        let final_response = response_builder.body(Full::new(response_body)).unwrap();

        if let (Some(cache), Some(key)) = (cache, cache_key) {
            let cache_response = final_response.clone();
            let cache_key_owned = key.clone();
            let cache_clone = cache.clone();

            tokio::spawn(async move {
                if let Err(e) = cache_clone.put(cache_key_owned, cache_response).await {
                    warn!("Failed to cache response: {}", e);
                }
            });
        }

        Ok(final_response)
    }
}

impl Default for ProxyHandler {
    fn default() -> Self {
        Self::new()
    }
}
