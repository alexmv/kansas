use crate::{configuration::RuntimeConfig, handler::MainService};
use arc_swap::ArcSwap;
use futures::TryFutureExt;
use hyper::server::conn::AddrStream;
use hyper::{service::make_service_fn, Server};
use std::{io, sync::Arc};

pub async fn create(config: Arc<ArcSwap<RuntimeConfig>>) -> Result<(), io::Error> {
    let address = config.load().listen_address;
    let service = make_service_fn(move |stream: &AddrStream| {
        let client_address = stream.remote_addr();
        let config = config.clone();

        async move {
            Ok::<_, io::Error>(MainService {
                client_address,
                config,
            })
        }
    });
    Server::bind(&address)
        .serve(service)
        .map_err(|e| {
            let msg = format!("Failed to listen server: {}", e);
            io::Error::new(io::ErrorKind::Other, msg)
        })
        .await
}
