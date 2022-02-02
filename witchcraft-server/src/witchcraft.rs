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
use crate::blocking::conjure::ConjureBlockingEndpoint;
use crate::blocking::pool::ThreadPool;
use crate::endpoint::conjure::ConjureEndpoint;
use crate::endpoint::extended_path::ExtendedPathEndpoint;
use crate::endpoint::WitchcraftEndpoint;
use crate::{blocking, RequestBody, ResponseWriter};
use conjure_http::server::{AsyncEndpoint, AsyncService, Endpoint, Service};
use conjure_runtime::ClientFactory;
use std::sync::Arc;
use tokio::runtime::Handle;
use witchcraft_metrics::MetricRegistry;
use witchcraft_server_config::install::InstallConfig;

/// The Witchcraft server context.
pub struct Witchcraft {
    pub(crate) metrics: Arc<MetricRegistry>,
    pub(crate) client_factory: ClientFactory,
    pub(crate) handle: Handle,
    pub(crate) install_config: InstallConfig,
    pub(crate) thread_pool: Option<Arc<ThreadPool>>,
    pub(crate) endpoints: Vec<Box<dyn WitchcraftEndpoint + Sync + Send>>,
}

impl Witchcraft {
    /// Returns a reference to the server's metric registry.
    #[inline]
    pub fn metrics(&self) -> &Arc<MetricRegistry> {
        &self.metrics
    }

    /// Returns a reference to the server's HTTP client factory.
    #[inline]
    pub fn client_factory(&self) -> &ClientFactory {
        &self.client_factory
    }

    /// Returns a reference to a handle to the server's Tokio runtime.
    #[inline]
    pub fn handle(&self) -> &Handle {
        &self.handle
    }

    /// Installs an async service at the server's root.
    pub fn app<T>(&mut self, service: T)
    where
        T: AsyncService<RequestBody, ResponseWriter>,
    {
        self.endpoints(None, service.endpoints())
    }

    /// Installs an async service under the server's `/api` prefix.
    pub fn api<T>(&mut self, service: T)
    where
        T: AsyncService<RequestBody, ResponseWriter>,
    {
        self.endpoints(Some("/api"), service.endpoints())
    }

    fn endpoints(
        &mut self,
        prefix: Option<&str>,
        endpoints: Vec<Box<dyn AsyncEndpoint<RequestBody, ResponseWriter> + Sync + Send>>,
    ) {
        self.endpoints.extend(
            endpoints
                .into_iter()
                .map(|e| Box::new(ConjureEndpoint::new(Some(&self.metrics), e)))
                .map(|e| extend_path(e, self.install_config.context_path(), prefix)),
        )
    }

    /// Installs a blocking service at the server's root.
    pub fn blocking_app<T>(&mut self, service: T)
    where
        T: Service<blocking::RequestBody, blocking::ResponseWriter>,
    {
        self.blocking_endpoints(None, service.endpoints())
    }

    /// Installs a blocking service under the server's `/api` prefix.
    pub fn blocking_api<T>(&mut self, service: T)
    where
        T: Service<blocking::RequestBody, blocking::ResponseWriter>,
    {
        self.blocking_endpoints(Some("/api"), service.endpoints())
    }

    fn blocking_endpoints(
        &mut self,
        prefix: Option<&str>,
        endpoints: Vec<
            Box<dyn Endpoint<blocking::RequestBody, blocking::ResponseWriter> + Sync + Send>,
        >,
    ) {
        let thread_pool = self
            .thread_pool
            .get_or_insert_with(|| Arc::new(ThreadPool::new(&self.install_config, &self.metrics)));

        self.endpoints.extend(
            endpoints
                .into_iter()
                .map(|e| Box::new(ConjureBlockingEndpoint::new(&self.metrics, thread_pool, e)))
                .map(|e| extend_path(e, self.install_config.context_path(), prefix)),
        )
    }
}

fn extend_path(
    endpoint: Box<dyn WitchcraftEndpoint + Sync + Send>,
    context_path: &str,
    prefix: Option<&str>,
) -> Box<dyn WitchcraftEndpoint + Sync + Send> {
    let context_path = if context_path == "/" {
        ""
    } else {
        context_path
    };
    let prefix = format!("{context_path}{}", prefix.unwrap_or(""));

    if prefix.is_empty() {
        endpoint
    } else {
        Box::new(ExtendedPathEndpoint::new(endpoint, &prefix))
    }
}
