use crate::{configuration::RuntimeConfig, handler::MainService};
use arc_swap::ArcSwap;
use dashmap::DashMap;
use futures::TryFutureExt;
use hyper::server::conn::AddrStream;
use hyper::{service::make_service_fn, Server};
use std::{io, sync::Arc};

pub async fn create(config: Arc<ArcSwap<RuntimeConfig>>) -> Result<(), io::Error> {
    let queue_map: Arc<DashMap<String, u16>> = Arc::new(DashMap::new());
    let address = config.load().listen_address;
    let service = make_service_fn(move |stream: &AddrStream| {
        let client_address = stream.remote_addr();
        let config = Arc::clone(&config);
        let queue_map = Arc::clone(&queue_map);

        async move {
            Ok::<_, io::Error>(MainService {
                client_address,
                config,
                queue_map,
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
