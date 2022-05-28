use crate::RuntimeConfig;
use arc_swap::ArcSwap;
use futures::future::join_all;
use hyper::{
    client::HttpConnector,
    http::uri::{self, Authority},
    Body, Client, Response, Result, StatusCode, Uri,
};
use hyper_timeout::TimeoutConnector;
use log::info;
use serde::Deserialize;
use std::{
    fmt::{self, Debug},
    str::FromStr,
    sync::Arc,
    time::Duration,
};
use tokio::time::interval;

#[derive(Debug, Deserialize, PartialEq, Eq)]
pub struct HealthConfig {
    pub timeout: Duration,
    pub interval: Duration,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Healthiness {
    Healthy,
    Unresponsive(Option<StatusCode>),
}

impl fmt::Display for Healthiness {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Healthiness::Healthy => write!(f, "Healthy"),
            Healthiness::Unresponsive(Some(status_code)) => {
                write!(f, "Unresponsive, status: {}", status_code)
            }
            Healthiness::Unresponsive(None) => write!(f, "Unresponsive"),
        }
    }
}

pub async fn watch_health(config: Arc<ArcSwap<RuntimeConfig>>) {
    let mut interval_timer = interval(config.load().backend.health_config.interval);
    loop {
        interval_timer.tick().await;
        let config = config.load();
        let mut checks = Vec::new();
        for (server_address, healthiness) in &config.backend.addresses {
            let future = check_server_health_once(
                server_address.clone(),
                healthiness,
                &config.backend.health_config,
            );
            checks.push(future);
        }
        join_all(checks).await;
    }
}

/* Contacts one server and sets health value if changed */
async fn check_server_health_once(
    server_address: String,
    healthiness: &ArcSwap<Healthiness>,
    health_config: &HealthConfig,
) {
    let uri = uri::Uri::builder()
        .scheme("http")
        .path_and_query(&health_config.path)
        .authority(Authority::from_str(&server_address).unwrap())
        .build()
        .unwrap();

    let result = contact_server(uri, health_config.timeout).await;
    update_health(&server_address, &result, healthiness, true)
}

async fn contact_server(server_address: Uri, timeout: Duration) -> Result<Response<Body>> {
    let http_connector = HttpConnector::new();
    let mut connector = TimeoutConnector::new(http_connector);
    connector.set_connect_timeout(Some(timeout));
    connector.set_read_timeout(Some(timeout));
    connector.set_write_timeout(Some(timeout));
    let client = Client::builder().build::<_, hyper::Body>(connector);

    client.get(server_address).await
}

pub fn update_health(
    server_address: &str,
    result: &Result<Response<Body>>,
    healthiness: &ArcSwap<Healthiness>,
    strict: bool,
) {
    let result = match result {
        Err(_) => Healthiness::Unresponsive(None),
        Ok(response) => {
            if response.status().is_success() {
                Healthiness::Healthy
            } else if response.status().is_client_error() && !strict {
                return;
            } else {
                Healthiness::Unresponsive(Some(response.status()))
            }
        }
    };

    let previous_healthiness = healthiness.load();
    if previous_healthiness.as_ref() != &result {
        info!("Backend health change for {}: {}", &server_address, &result);
        healthiness.store(Arc::new(result));
    }
}
