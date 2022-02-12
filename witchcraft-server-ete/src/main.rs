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
use crate::conjure::TestServiceEndpoints;
use conjure_error::Error;
use refreshable::Refreshable;
use std::env;
use witchcraft_server::config::install::InstallConfig;
use witchcraft_server::config::runtime::RuntimeConfig;
use witchcraft_server::Witchcraft;

mod async_handler;
mod handler;

#[allow(dead_code, warnings)]
mod conjure {
    include!(concat!(env!("OUT_DIR"), "/conjure/mod.rs"));
}

fn main() {
    witchcraft_server::init(init);
}

fn init(
    _: InstallConfig,
    _: Refreshable<RuntimeConfig, Error>,
    wc: &mut Witchcraft,
) -> Result<(), Error> {
    match &*env::var("HANDLER_TYPE").unwrap() {
        "async" => {
            wc.api(TestServiceEndpoints::new(async_handler::TestResource));
        }
        "blocking" => {
            wc.blocking_api(TestServiceEndpoints::new(handler::TestResource));
        }
        ty => panic!("invalid handler type {ty}"),
    }

    Ok(())
}
