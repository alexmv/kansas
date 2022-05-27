use crate::{
    configuration::RuntimeConfig,
    error_response::{bad_gateway, log_error},
    health::{update_health, HealthConfig, Healthiness},
};
use arc_swap::ArcSwap;
use futures::Future;
use hyper::{
    client::HttpConnector, header::HeaderValue, service::Service, Body, Client, Request, Response,
    Uri,
};
use log::info;
use rand::{thread_rng, Rng};
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};

pub struct MainService {
    pub client_address: SocketAddr,
    pub config: Arc<ArcSwap<RuntimeConfig>>,
}

impl Service<Request<Body>> for MainService {
    type Response = Response<Body>;
    type Error = hyper::Error;

    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: Request<Body>) -> Self::Future {
        info!(
            "{:#?} {} {}",
            request.version(),
            request.method(),
            request.uri()
        );

        let config = self.config.load();

        let pool = config.backend.clone();
        let client_address = self.client_address;

        Box::pin(async move {
            let working_addresses = pool
                .addresses
                .iter()
                .filter(|(_, healthiness)| healthiness.load().as_ref() == &Healthiness::Healthy)
                .map(|(address, _)| address.as_str())
                .collect::<Vec<_>>();

            if working_addresses.is_empty() {
                return Ok(bad_gateway());
            }
            let backend_address = choose_backend(&working_addresses, &request);
            let result =
                forward_request_to_backend(backend_address, request, &client_address, pool.clone())
                    .await;
            Ok(result)
        })
    }
}

fn choose_backend<'l>(backend_addresses: &'l [&str], _request: &Request<Body>) -> &'l str {
    let mut rng = thread_rng();
    let index = rng.gen_range(0..backend_addresses.len());
    backend_addresses[index]
}

fn append_forwarded_for(existing_forwarded_for: Option<&HeaderValue>, client_ip: String) -> String {
    match existing_forwarded_for {
        Some(existing_forwarded_for) => {
            let mut forwarded_for = existing_forwarded_for.to_str().unwrap_or("").to_owned();
            forwarded_for.push_str(&format!(", {}", &client_ip));
            forwarded_for
        }
        None => client_ip,
    }
}

async fn forward_request_to_backend(
    backend_address: &str,
    request: Request<Body>,
    client_address: &SocketAddr,
    pool: Arc<BackendPool>,
) -> Response<Body> {
    let path = request.uri().path_and_query().unwrap().clone();
    let url = Uri::builder()
        .scheme("http")
        .authority(backend_address)
        .path_and_query(path)
        .build()
        .unwrap();

    let builder = Request::builder().uri(url);

    // IPv4 addresses may show as as their IPv4-in-IPv6 equivalent, like `::ffff:127.0.0.1`
    let ip = match client_address.ip() {
        IpAddr::V4(v4) => v4.to_string(),
        IpAddr::V6(v6) => v6
            .to_ipv4()
            .map(|v4| v4.to_string())
            .unwrap_or_else(|| v6.to_string()),
    };
    let builder = request
        .headers()
        .iter()
        .fold(builder, |builder, (key, val)| builder.header(key, val))
        .header(
            "x-forwarded-for",
            append_forwarded_for(request.headers().get("x-forwarded-for"), ip),
        )
        .method(request.method());

    let backend_request = builder.body(request.into_body()).unwrap();

    let result = pool.client.request(backend_request).await;

    // Update the backend state
    let healthiness = pool.addresses.get(backend_address).unwrap();
    update_health(backend_address, &result, healthiness, false);

    // 502 on errors
    match result {
        Err(error) => {
            log_error(error);
            bad_gateway()
        }
        Ok(res) => res,
    }
}

#[derive(Debug)]
pub struct BackendPool {
    pub addresses: HashMap<String, ArcSwap<Healthiness>>,
    pub health_config: HealthConfig,
    pub client: Client<HttpConnector, Body>,
}

pub struct BackendPoolBuilder {
    addresses: HashMap<String, ArcSwap<Healthiness>>,
    health_config: HealthConfig,
    pool_idle_timeout: Option<Duration>,
    pool_max_idle_per_host: Option<usize>,
}

impl BackendPoolBuilder {
    pub fn new(
        addresses: HashMap<String, ArcSwap<Healthiness>>,
        health_config: HealthConfig,
    ) -> BackendPoolBuilder {
        BackendPoolBuilder {
            addresses,
            health_config,
            pool_idle_timeout: None,
            pool_max_idle_per_host: None,
        }
    }

    pub fn pool_idle_timeout(&mut self, duration: Duration) -> &BackendPoolBuilder {
        self.pool_idle_timeout = Some(duration);
        self
    }

    pub fn pool_max_idle_per_host(&mut self, max_idle: usize) -> &BackendPoolBuilder {
        self.pool_max_idle_per_host = Some(max_idle);
        self
    }

    pub fn build(self) -> BackendPool {
        let mut client_builder = Client::builder();
        if let Some(pool_idle_timeout) = self.pool_idle_timeout {
            client_builder.pool_idle_timeout(pool_idle_timeout);
        }
        if let Some(pool_max_idle_per_host) = self.pool_max_idle_per_host {
            client_builder.pool_max_idle_per_host(pool_max_idle_per_host);
        }

        let client: Client<_, Body> = client_builder.build(HttpConnector::new());

        BackendPool {
            addresses: self.addresses,
            health_config: self.health_config,
            client,
        }
    }
}
