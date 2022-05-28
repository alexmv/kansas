use crate::{
    handler::{BackendPool, BackendPoolBuilder},
    health::{HealthConfig, Healthiness},
};
use arc_swap::ArcSwap;
use serde::Deserialize;
use std::{
    error::Error, fmt::Debug, fs, io, net::SocketAddr, path::Path, sync::Arc, time::Duration,
};

pub async fn read_initial_config<P: AsRef<Path>>(
    path: P,
) -> Result<Arc<ArcSwap<RuntimeConfig>>, io::Error> {
    let config = read_runtime_config(&path).await.map_err(|e| {
        io::Error::new(
            e.kind(),
            format!("Could not load configuration due to: {}", e),
        )
    })?;
    Ok(Arc::new(ArcSwap::from_pointee(config)))
}

async fn read_runtime_config<P>(path: P) -> Result<RuntimeConfig, io::Error>
where
    P: AsRef<Path>,
{
    let config = TomlConfig::read(&path)?;
    let listen_address = config.listen_address.parse().map_err(invalid_data)?;

    Ok(RuntimeConfig {
        listen_address,
        backend: config.backend.into(),
    })
}

fn invalid_data<E>(error: E) -> io::Error
where
    E: Into<Box<dyn Error + Send + Sync>>,
{
    io::Error::new(io::ErrorKind::InvalidData, error)
}

pub struct RuntimeConfig {
    pub listen_address: SocketAddr,
    pub backend: BackendPool,
}

#[derive(Debug, Deserialize)]
struct TomlConfig {
    #[serde(default = "default_listen_address")]
    listen_address: String,
    backend: BackendPoolConfig,
}

fn default_listen_address() -> String {
    "127.0.0.1:9799".to_string()
}

impl TomlConfig {
    fn read<P: AsRef<Path>>(toml_path: P) -> io::Result<TomlConfig> {
        let toml_str = fs::read_to_string(&toml_path).map_err(|e| {
            io::Error::new(
                e.kind(),
                format!(
                    "Error occurred when reading configuration file {}: {}",
                    toml_path.as_ref().display(),
                    e
                ),
            )
        })?;
        let config: TomlConfig = toml::from_str(&toml_str).map_err(|e| {
            let e = io::Error::from(e);
            io::Error::new(
                e.kind(),
                format!(
                    "Error occurred when parsing configuration file {}: {}",
                    toml_path.as_ref().display(),
                    e
                ),
            )
        })?;
        Ok(config)
    }
}

#[derive(Debug, Deserialize)]
struct BackendPoolConfig {
    addresses: Vec<String>,
    client: Option<BackendConnectionConfig>,
    #[serde(default = "default_health_config")]
    health_config: HealthTomlConfig,
}

impl From<BackendPoolConfig> for BackendPool {
    fn from(other: BackendPoolConfig) -> Self {
        // TODO: This conversion can fail, should we use TryFrom or wrap this in some kind of error?
        let addresses = other
            .addresses
            .into_iter()
            .map(|address| (address, ArcSwap::from_pointee(Healthiness::Healthy)))
            .collect();
        let health_toml_config = other.health_config;

        let health_config = HealthConfig {
            timeout: health_toml_config.timeout,
            interval: health_toml_config.interval,
            path: health_toml_config.path,
        };

        let mut builder = BackendPoolBuilder::new(addresses, health_config);
        if let Some(client) = other.client {
            if let Some(pool_idle_timeout) = client.pool_idle_timeout {
                builder.pool_idle_timeout(pool_idle_timeout);
            }

            if let Some(pool_max_idle_per_host) = client.pool_max_idle_per_host {
                builder.pool_max_idle_per_host(pool_max_idle_per_host);
            }
        }

        builder.build()
    }
}

#[derive(Debug, Deserialize)]
struct BackendConnectionConfig {
    pool_idle_timeout: Option<Duration>,
    pool_max_idle_per_host: Option<usize>,
}

#[derive(Debug, Deserialize, PartialEq, Eq, Default)]
pub struct HealthTomlConfig {
    #[serde(default = "default_timeout", with = "humantime_serde")]
    pub timeout: Duration,
    #[serde(default = "default_interval", with = "humantime_serde")]
    pub interval: Duration,
    #[serde(default = "default_path")]
    pub path: String,
}

fn default_health_config() -> HealthTomlConfig {
    HealthTomlConfig {
        timeout: default_timeout(),
        interval: default_interval(),
        path: default_path(),
    }
}

fn default_timeout() -> Duration {
    Duration::from_millis(500)
}

fn default_interval() -> Duration {
    Duration::from_secs(5)
}

fn default_path() -> String {
    "/".to_string()
}
