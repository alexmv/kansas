use std::future::Future;
use std::time::Instant;

use hyper::{Body, Error, Method, Response, StatusCode};
use prometheus::{
    register_histogram_vec, register_int_counter_vec, register_int_gauge, HistogramVec,
    IntCounterVec, IntGauge, Opts, TextEncoder,
};

lazy_static! {
    pub static ref OPEN_CONNECTIONS: IntGauge =
        register_int_gauge!("kansas_open_connections_total", "Current open connections").unwrap();
    pub static ref REQUESTS: IntCounterVec = register_int_counter_vec!(
        Opts::new("kansas_requests_total", "Total requests"),
        &["method"]
    )
    .unwrap();
    pub static ref RESPONSES: IntCounterVec = register_int_counter_vec!(
        Opts::new("kansas_responses_total", "Total response codes"),
        &["method", "status"]
    )
    .unwrap();
    pub static ref RESPONSE_TIME: HistogramVec = register_histogram_vec!(
        "kansas_response_time_seconds",
        "Response times",
        &["method", "status"],
        vec![0.0, 0.01, 0.02, 0.05, 0.1, 0.2, 0.5, 1.0, 10.0, 20.0, 30.0, 40.0, 50.0, 60.0, 70.0],
    )
    .unwrap();
}

use prometheus::core::{Atomic, GenericGauge, Number};

pub struct GenericGaugeGuard<P: Atomic + 'static> {
    value: P::T,
    gauge: &'static GenericGauge<P>,
}
impl<P: Atomic + 'static> Drop for GenericGaugeGuard<P> {
    fn drop(&mut self) {
        self.gauge.sub(self.value);
    }
}

pub trait GuardedGauge<P: Atomic + 'static> {
    #[must_use]
    fn guarded_inc(&'static self) -> GenericGaugeGuard<P>;
}

impl<P: Atomic + 'static> GuardedGauge<P> for GenericGauge<P> {
    fn guarded_inc(&'static self) -> GenericGaugeGuard<P> {
        self.inc();
        GenericGaugeGuard {
            value: <P::T as Number>::from_i64(1),
            gauge: self,
        }
    }
}

pub fn handler() -> Result<Response<Body>, Error> {
    let encoder = TextEncoder::new();
    let metric_families = prometheus::gather();
    let txt = encoder.encode_to_string(&metric_families).unwrap();

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "text/plain")
        .body(Body::from(txt))
        .unwrap())
}

pub async fn instrumented<Fut>(method: Method, inner: Fut) -> Result<Response<Body>, Error>
where
    Fut: Future<Output = Result<Response<Body>, Error>>,
{
    REQUESTS.with_label_values(&[method.as_str()]).inc();
    let _guard = OPEN_CONNECTIONS.guarded_inc();
    let now = Instant::now();
    let result = inner.await;
    if let Ok(ref response) = result {
        let status = response.status().as_u16().to_string();
        let attrs = &[method.as_str(), status.as_str()];
        RESPONSES.with_label_values(attrs).inc();
        RESPONSE_TIME
            .with_label_values(attrs)
            .observe(now.elapsed().as_secs_f64());
    }
    result
}
