// Copyright 2022 Palantir Technologies, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
use crate::service::accept::AcceptService;
use crate::service::connection_limit::ConnectionLimitLayer;
use crate::service::connection_metrics::ConnectionMetricsLayer;
use crate::service::handler::HandlerService;
use crate::service::hyper::HyperService;
use crate::service::idle_connection::IdleConnectionLayer;
use crate::service::request_id::RequestIdLayer;
use crate::service::routing::RoutingLayer;
use crate::service::spans::{SpannedBody, SpansLayer};
use crate::service::tls::TlsLayer;
use crate::service::tls_metrics::TlsMetricsLayer;
use crate::service::trace_propagation::TracePropagationLayer;
use crate::service::{Service, ServiceBuilder};
use crate::Witchcraft;
use conjure_error::Error;
use std::sync::Arc;
use tokio::task;
use witchcraft_log::debug;
use witchcraft_server_config::install::InstallConfig;

pub type RawBody = SpannedBody<hyper::Body>;

pub async fn start(config: &InstallConfig, witchcraft: &mut Witchcraft) -> Result<(), Error> {
    // This service handles invididual HTTP requests, each running concurrently.
    let request_service = ServiceBuilder::new()
        .layer(RoutingLayer::new(vec![]))
        .layer(RequestIdLayer)
        .layer(TracePropagationLayer)
        .layer(SpansLayer)
        .service(HandlerService);

    // This layer handles invididual TCP connections, each running concurrently.
    let handle_service = ServiceBuilder::new()
        .layer(TlsLayer::new(config)?)
        .layer(TlsMetricsLayer::new(&witchcraft.metrics))
        .layer(IdleConnectionLayer::new(config))
        .service(HyperService::new(request_service));
    let handle_service = Arc::new(handle_service);

    // This layer produces TCP connections, running serially.
    let accept_service = ServiceBuilder::new()
        .layer(ConnectionLimitLayer::new(config))
        .layer(ConnectionMetricsLayer::new(config, &witchcraft.metrics))
        .service(AcceptService::new(config)?);

    task::spawn(async move {
        loop {
            let socket = accept_service.call(()).await;

            task::spawn({
                let handle_service = handle_service.clone();
                async move {
                    if let Err(e) = handle_service.call(socket).await {
                        debug!("http connection terminated", error: e);
                    }
                }
            });
        }
    });

    Ok(())
}
