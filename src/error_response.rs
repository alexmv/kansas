use hyper::{Body, Response, StatusCode};
use log::error;
use std::error::Error;

pub fn bad_gateway() -> Response<Body> {
    Response::builder()
        .status(StatusCode::BAD_GATEWAY)
        .body(Body::empty())
        .unwrap()
}

pub fn log_error<E: Error>(error: E) {
    error!("{}", error);
}
