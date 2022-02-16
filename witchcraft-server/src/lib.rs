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
//! A highly opinionated embedded application server for RESTy APIs.
//!
//! # Configuration
//!
//! The configuration for a Witchcraft server is split into two files. `install.yml` contains the configuration that is
//! fixed at server startup, and `runtime.yml` contains the configuration that can be updated dynamically at runtime.
//! These are deserialized into Rust types via the [`serde::Deserialize`] trait. Witchcraft's own internal configuration
//! is represented by the [`InstallConfig`] and [`RuntimeConfig`] types. Services that need their own configuration
//! should embed the Witchcraft configuration within their own using the `#[serde(flatten)]` annotation and implement
//! the [`AsRef`] trait:
//!
//! ```
//! use serde::Deserialize;
//! use witchcraft_server::config::install::InstallConfig;
//!
//! #[derive(Deserialize)]
//! #[serde(rename_all = "kebab-case")]
//! struct MyInstallConfig {
//!     shave_yaks: bool,
//!     #[serde(flatten)]
//!     base: InstallConfig,
//! }
//!
//! impl AsRef<InstallConfig> for MyInstallConfig {
//!     fn as_ref(&self) -> &InstallConfig {
//!         &self.base
//!     }
//! }
//! ```
//!
//! The service's custom configuration will then sit next to the standard Witchcraft configuration in `install.yml`:
//!
//! ```yml
//! product-name: my-service
//! product-version: 1.0.0
//! port: 12345
//! shave-yaks: true
//! ```
#![warn(missing_docs)]

use crate::health::config_reload::ConfigReloadHealthCheck;
use crate::health::endpoint_500s::Endpoint500sHealthCheck;
use crate::health::panics::PanicsHealthCheck;
use crate::health::service_dependency::ServiceDependencyHealthCheck;
use crate::health::HealthCheckRegistry;
use crate::readiness::ReadinessCheckRegistry;
use crate::shutdown_hooks::ShutdownHooks;
use crate::status::StatusEndpoints;
pub use body::{RequestBody, ResponseWriter};
use config::install::InstallConfig;
use config::runtime::RuntimeConfig;
use conjure_error::Error;
use conjure_http::server::AsyncService;
use conjure_object::Utc;
use conjure_runtime::{Agent, ClientFactory, HostMetricsRegistry, UserAgent};
use futures_util::{stream, Stream, StreamExt};
use refreshable::Refreshable;
use serde::de::DeserializeOwned;
use std::process;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Runtime;
use tokio::signal::unix::{self, SignalKind};
use tokio::{pin, runtime, select, time};
pub use witchcraft::Witchcraft;
use witchcraft_log::{fatal, info};
use witchcraft_metrics::MetricRegistry;
#[doc(inline)]
pub use witchcraft_server_config as config;

pub mod blocking;
mod body;
mod configs;
mod endpoint;
pub mod health;
mod logging;
mod metrics;
pub mod readiness;
mod server;
mod service;
mod shutdown_hooks;
mod status;
pub mod tls;
mod witchcraft;

/// Initializes a Witchcraft server.
///
/// `init` is invoked with the parsed install and runtime configs as well as the [`Witchcraft`] context object. It
/// is expected to return quickly; any long running initialization should be spawned off into the background to run
/// asynchronously.
pub fn init<I, R, F>(init: F)
where
    I: AsRef<InstallConfig> + DeserializeOwned,
    R: AsRef<RuntimeConfig> + DeserializeOwned + PartialEq + 'static + Sync + Send,
    F: FnOnce(I, Refreshable<R, Error>, &mut Witchcraft) -> Result<(), Error>,
{
    match init_inner(init) {
        Ok(()) => (),
        Err(e) => {
            // we don't know if logging has been initialized, so both log and print the error.
            fatal!("error starting server", error: e);
            eprintln!(
                "[{}] - {} safe: {:?} unsafe: {:?}",
                Utc::now(),
                e.cause(),
                e.safe_params(),
                e.unsafe_params(),
            );
            for backtrace in e.backtraces() {
                eprintln!("{:?}", backtrace);
            }
            process::exit(1)
        }
    }
}

fn init_inner<I, R, F>(init: F) -> Result<(), Error>
where
    I: AsRef<InstallConfig> + DeserializeOwned,
    R: AsRef<RuntimeConfig> + DeserializeOwned + PartialEq + 'static + Sync + Send,
    F: FnOnce(I, Refreshable<R, Error>, &mut Witchcraft) -> Result<(), Error>,
{
    let install_config = configs::load_install::<I>()?;

    let thread_id = AtomicUsize::new(0);
    let runtime = runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name_fn(move || format!("runtime-{}", thread_id.fetch_add(1, Ordering::Relaxed)))
        .worker_threads(install_config.as_ref().server().io_threads())
        .thread_keep_alive(install_config.as_ref().server().idle_thread_timeout())
        .build()
        .map_err(Error::internal_safe)?;

    let handle = runtime.handle().clone();
    // ensure that we exit quickly even if there are stuck blocking tasks
    let _runtime = QuickDropRuntime {
        runtime: Some(runtime),
    };

    let runtime_config_ok = Arc::new(AtomicBool::new(true));
    let runtime_config = configs::load_runtime::<R>(&handle, &runtime_config_ok)?;

    let metrics = Arc::new(MetricRegistry::new());

    let mut logger_shutdown_hooks = ShutdownHooks::new();

    let loggers = handle.block_on(logging::init(
        &metrics,
        install_config.as_ref(),
        &runtime_config.map(|c| c.as_ref().logging().clone()),
        &mut logger_shutdown_hooks,
    ))?;

    info!("server starting");

    metrics::init(&metrics);

    let host_metrics = Arc::new(HostMetricsRegistry::new());

    let health_checks = Arc::new(HealthCheckRegistry::new(&handle));
    health_checks.register(ServiceDependencyHealthCheck::new(&host_metrics));
    health_checks.register(PanicsHealthCheck::new());
    health_checks.register(ConfigReloadHealthCheck::new(runtime_config_ok));

    let readiness_checks = Arc::new(ReadinessCheckRegistry::new());

    let mut client_factory =
        ClientFactory::new(runtime_config.map(|c| c.as_ref().service_discovery().clone()));
    client_factory
        .user_agent(UserAgent::new(Agent::new(
            install_config.as_ref().product_name(),
            install_config.as_ref().product_version(),
        )))
        .metrics(metrics.clone())
        .host_metrics(host_metrics)
        .blocking_handle(handle.clone());

    let mut witchcraft = Witchcraft {
        metrics,
        health_checks,
        readiness_checks,
        client_factory,
        handle: handle.clone(),
        install_config: install_config.as_ref().clone(),
        thread_pool: None,
        endpoints: vec![],
    };

    let status_endpoints = StatusEndpoints::new(
        &runtime_config,
        &witchcraft.health_checks,
        &witchcraft.readiness_checks,
    );
    witchcraft.endpoints(None, status_endpoints.endpoints(), false);

    init(install_config, runtime_config, &mut witchcraft)?;

    witchcraft
        .health_checks
        .register(Endpoint500sHealthCheck::new(&witchcraft.endpoints));

    let mut server_shutdown_hooks = ShutdownHooks::new();
    handle.block_on(server::start(
        &mut witchcraft,
        &mut server_shutdown_hooks,
        loggers.request_logger,
    ))?;

    handle.block_on(shutdown(
        logger_shutdown_hooks,
        server_shutdown_hooks,
        witchcraft.install_config.server().shutdown_timeout(),
    ))
}

async fn shutdown(
    logger_shutdown_hooks: ShutdownHooks,
    server_shutdown_hooks: ShutdownHooks,
    timeout: Duration,
) -> Result<(), Error> {
    pin! {
        let signals = signals()?;
    }

    signals.next().await;
    info!("server shutting down");

    select! {
        _ = server_shutdown_hooks => {}
        _ = signals.next() => info!("graceful shutdown interrupted by signal"),
        _ = time::sleep(timeout) => {
            info!(
                "graceful shutdown timed out",
                safe: {
                    timeout: format_args!("{timeout:?}"),
                },
            );
        }
    }

    logger_shutdown_hooks.await;

    Ok(())
}

fn signals() -> Result<impl Stream<Item = ()>, Error> {
    let sigint = signal(SignalKind::interrupt())?;
    let sigterm = signal(SignalKind::terminate())?;
    Ok(stream::select(sigint, sigterm))
}

fn signal(kind: SignalKind) -> Result<impl Stream<Item = ()>, Error> {
    let mut signal = unix::signal(kind).map_err(Error::internal_safe)?;
    Ok(stream::poll_fn(move |cx| signal.poll_recv(cx)))
}

struct QuickDropRuntime {
    runtime: Option<Runtime>,
}

impl Drop for QuickDropRuntime {
    fn drop(&mut self) {
        self.runtime.take().unwrap().shutdown_background()
    }
}
