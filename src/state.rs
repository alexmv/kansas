use crate::{handler::BackendPool, health::Healthiness};
use anyhow::Result;
use bytes::{Bytes, BytesMut};
use dashmap::DashMap;
use hyper::body::HttpBody;
use hyper::{Body, Method, Request, Response};
use log::{debug, info};
use thiserror::Error;
use url::form_urlencoded;

use std::sync::Arc;

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

async fn get_port(
    queue_map: Arc<DashMap<String, u16>>,
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
        let buffer = match request.method().as_str() {
            "DELETE" => {
                // We need to consume the body from the request, while
                // also leaving it to be sent to the backend
                let body = request.body_mut();
                let mut buf = BytesMut::with_capacity(body.size_hint().lower() as usize);
                while let Some(chunk) = body.data().await {
                    buf.extend_from_slice(&chunk.map_err(|_| {
                        BadBackendError::BadRequest("Failed to read request body".into())
                    })?);
                }
                let buf = buf.freeze();
                *request.body_mut() = Body::from(buf.clone());
                buf
            }
            "GET" => Bytes::from(
                request
                    .uri()
                    .query()
                    .ok_or_else(|| BadBackendError::BadRequest("No query string".into()))?
                    .to_owned(),
            ),
            _ => {
                return Err(BadBackendError::BadRequest(format!(
                    "Unknown method {}",
                    request.method()
                )))
            }
        };
        let queue_id = form_urlencoded::parse(&buffer)
            .into_owned()
            .find(|pair| pair.0 == "queue_id")
            .ok_or_else(|| BadBackendError::UnknownQueue("(missing)".into()))?
            .1;
        let queue_backend = queue_map
            .get(queue_id.as_str())
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
    pool: Arc<BackendPool>,
    queue_map: Arc<DashMap<String, u16>>,
    request: &mut Request<Body>,
) -> Result<(u16, String), BadBackendError> {
    let port = get_port(queue_map, request).await?;
    let backend = format!("127.0.0.1:{}", port);
    let health = pool
        .addresses
        .get(backend.as_str())
        .ok_or_else(|| BadBackendError::UnknownHost(backend.clone()))?;
    if health.load().as_ref() != &Healthiness::Healthy {
        // Backend is down, stall for time?
        Err(BadBackendError::UnhealthyHost(backend))
    } else {
        Ok((port, backend))
    }
}

pub fn store_backend(
    queue_map: Arc<DashMap<String, u16>>,
    method: Method,
    resp: &Response<Body>,
    port: u16,
) {
    if resp.status().is_success() {
        if let Some(queue_header) = resp.headers().get("x-tornado-queue-id") {
            if let Ok(queue_id) = queue_header.to_str() {
                if method == "DELETE" {
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
