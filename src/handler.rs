use crate::{
    configuration::RuntimeConfig,
    error_response::{bad_gateway, bad_queue, log_error},
    health::{update_health, HealthConfig, Healthiness},
    metrics,
    state::{choose_backend, store_backend, BadBackendError},
};
use arc_swap::ArcSwap;
use dashmap::DashMap;
use futures::Future;
use hyper::{
    client::HttpConnector, header::HeaderValue, service::Service, Body, Client, Request, Response,
    Uri,
};
use log::info;
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
    pub config: Arc<RuntimeConfig>,
    pub queue_map: Arc<DashMap<String, u16>>,
}

impl Service<Request<Body>> for MainService {
    type Response = Response<Body>;
    type Error = hyper::Error;

    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, mut request: Request<Body>) -> Self::Future {
        info!(
            "{:#?} {} {}",
            request.version(),
            request.method(),
            request.uri()
        );

        if request.uri().path() == "/metrics" {
            return Box::pin(async move { metrics::handler() });
        }

        let config = Arc::clone(&self.config);

        let queue_map = Arc::clone(&self.queue_map);
        let client_address = self.client_address;

        Box::pin(metrics::instrumented(
            request.method().clone(),
            async move {
                let pool = &config.backend;
                let method = request.method().clone();
                let backend = choose_backend(pool, &queue_map, &mut request).await;
                match backend {
                    Ok((port, chosen_backend)) => {
                        let resp = forward_request_to_backend(
                            &chosen_backend,
                            request,
                            &client_address,
                            pool,
                        )
                        .await;
                        store_backend(&queue_map, method, &resp, port);
                        Ok(resp)
                    }
                    Err(BadBackendError::UnknownQueue(q)) => Ok(bad_queue(q)),
                    Err(error) => {
                        log_error(error);
                        Ok(bad_gateway())
                    }
                }
            },
        ))
    }
}

fn append_forwarded_for(existing_forwarded_for: Option<&HeaderValue>, client_ip: String) -> String {
    match existing_forwarded_for {
        Some(existing_forwarded_for) => {
            let forwarded_for = existing_forwarded_for.to_str().unwrap_or("");
            format!("{}, {}", forwarded_for, client_ip)
        }
        None => client_ip,
    }
}

async fn forward_request_to_backend(
    backend_address: &str,
    request: Request<Body>,
    client_address: &SocketAddr,
    pool: &BackendPool,
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
            .map_or_else(|| v6.to_string(), |v4| v4.to_string()),
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
