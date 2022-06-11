use crate::{handler::BackendPool, health::Healthiness};
use anyhow::Result;
use bytes::Bytes;
use dashmap::DashMap;
use hyper::{Body, Method, Request, Response};
use log::{debug, info};
use std::mem;
use thiserror::Error;
use url::form_urlencoded;

#[derive(Error, Debug)]
pub enum BadBackendError {
    #[error("Bad request: {0}")]
    BadRequest(String),

    #[error("Unhealthy backend: {0}")]
    UnhealthyHost(String),

    #[error("Unknown backend: {0}")]
    UnknownHost(String),

    #[error("Unknown queue-id: {0}")]
    UnknownQueue(String),
}

// This RAII wrapper streams a request body into memory so we can
// examine it; when the wrapper is dropped, we stuff the body back
// into the request so it can be forwarded to the backend.
struct PeekBody<'a> {
    bytes: Bytes,
    body: &'a mut Body,
}

impl PeekBody<'_> {
    async fn new(body: &mut Body) -> Result<PeekBody<'_>, hyper::Error> {
        Ok(PeekBody {
            bytes: hyper::body::to_bytes(&mut *body).await?,
            body,
        })
    }
}

impl Drop for PeekBody<'_> {
    fn drop(&mut self) {
        *self.body = Body::from(mem::take(&mut self.bytes));
    }
}

async fn get_port(
    queue_map: &DashMap<String, u16>,
    request: &mut Request<Body>,
) -> Result<u16, BadBackendError> {
    if request.uri().path() == "/api/v1/events/internal" {
        let port_header = request
            .headers()
            .get("x-tornado-shard")
            .ok_or_else(|| BadBackendError::BadRequest("No x-tornado-shard header".into()))?;
        let port_str = port_header
            .to_str()
            .map_err(|_| BadBackendError::BadRequest("Cannot convert header to string".into()))?;
        info!("Creating new queue on port {}", port_str);
        Ok(port_str
            .parse::<u16>()
            .map_err(|_| BadBackendError::BadRequest("Failed to parse port as int".into()))?)
    } else {
        let peek_body;
        let body_bytes = match *request.method() {
            Method::DELETE => {
                peek_body = PeekBody::new(request.body_mut()).await.map_err(|_| {
                    BadBackendError::BadRequest("Failed to read request body".into())
                })?;
                &peek_body.bytes
            }
            Method::GET => request
                .uri()
                .query()
                .ok_or_else(|| BadBackendError::BadRequest("No query string".into()))?
                .as_bytes(),
            _ => {
                return Err(BadBackendError::BadRequest(format!(
                    "Unknown method {}",
                    request.method()
                )))
            }
        };
        let queue_id = form_urlencoded::parse(body_bytes)
            .into_owned()
            .find(|pair| pair.0 == "queue_id")
            .ok_or_else(|| BadBackendError::UnknownQueue("(missing)".into()))?
            .1;
        let queue_backend = queue_map
            .get(&queue_id)
            .ok_or(BadBackendError::UnknownQueue(queue_id))?;
        debug!(
            "Routing queue {} to port {}",
            queue_backend.key(),
            *queue_backend
        );
        Ok(*queue_backend)
    }
}

pub async fn choose_backend(
    pool: &BackendPool,
    queue_map: &DashMap<String, u16>,
    request: &mut Request<Body>,
) -> Result<(u16, String), BadBackendError> {
    let port = get_port(queue_map, request).await?;
    let backend = format!("127.0.0.1:{}", port);
    let health = pool
        .addresses
        .get(&backend)
        .ok_or_else(|| BadBackendError::UnknownHost(backend.clone()))?;
    if **health.load() != Healthiness::Healthy {
        // Backend is down, stall for time?
        Err(BadBackendError::UnhealthyHost(backend))
    } else {
        Ok((port, backend))
    }
}

pub fn store_backend(
    queue_map: &DashMap<String, u16>,
    method: Method,
    resp: &Response<Body>,
    port: u16,
) {
    if resp.status().is_success() {
        if let Some(queue_header) = resp.headers().get("x-tornado-queue-id") {
            if let Ok(queue_id) = queue_header.to_str() {
                if method == Method::DELETE {
                    info!("Removed queue {} from port {}", queue_id, port);
                    queue_map.remove(queue_id).unwrap();
                } else {
                    info!("Created new queue {} on port {}", queue_id, port);
                    queue_map.insert(queue_id.to_string(), port);
                }
            }
        }
    }
}
