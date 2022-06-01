use hyper::{Body, Response, StatusCode};
use log::error;
use serde_json::json;
use std::error::Error;

pub fn bad_queue(q: String) -> Response<Body> {
    let resp = json!({
        "result": "error".to_string(),
        "msg": format!("Bad event queue_id: {}", q),
        "queue_id": q,
        "code": "BAD_EVENT_QUEUE_ID".to_string(),
    });
    Response::builder()
        .status(StatusCode::BAD_REQUEST)
        .header("Content-Type", "application/json")
        .body(Body::from(resp.to_string()))
        .unwrap()
}

pub fn bad_gateway() -> Response<Body> {
    Response::builder()
        .status(StatusCode::BAD_GATEWAY)
        .body(Body::empty())
        .unwrap()
}

pub fn log_error<E: Error>(error: E) {
    error!("{}", error);
}
