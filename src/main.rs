use arc_swap::ArcSwap;
use clap::{Arg, Command};
use configuration::{read_initial_config, RuntimeConfig};
use std::{io, sync::Arc};
use tokio::try_join;

mod configuration;
mod error_response;
mod handler;
mod health;
mod metrics;
mod server;
mod state;

#[macro_use]
extern crate lazy_static;

#[tokio::main]
pub async fn main() -> Result<(), io::Error> {
    let matches = Command::new("Kansas")
        .version("1.0")
        .about("Load-balance tornado servers")
        .arg(
            Arg::new("config")
                .short('c')
                .long("config")
                .value_name("TOML FILE")
                .help("The path to the configuration in TOML format.")
                .required(true)
                .takes_value(true),
        )
        .get_matches();
    let config_path = matches.value_of("config").unwrap().to_string();

    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    console_subscriber::init();

    let config = read_initial_config(&config_path).await?;
    try_join!(
        watch_health(config.clone()),
        listen_for_http_request(config.clone()),
    )?;
    Ok(())
}

async fn watch_health(config: Arc<ArcSwap<RuntimeConfig>>) -> Result<(), io::Error> {
    health::watch_health(config).await;
    Ok(())
}

async fn listen_for_http_request(config: Arc<ArcSwap<RuntimeConfig>>) -> Result<(), io::Error> {
    server::create(config).await
}
