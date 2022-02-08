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
use crate::metrics::rlimit::Rlimit;
use crate::metrics::rusage::Rusage;
use procinfo::pid;
use std::time::Instant;
use std::{fs, io, panic};
use witchcraft_log::info;
use witchcraft_metrics::MetricRegistry;

#[cfg(feature = "jemalloc")]
mod jemalloc;
mod rlimit;
mod rusage;

pub fn init(metrics: &MetricRegistry) {
    register_uptime_metric(metrics);
    register_panic_metric(metrics);
    register_rusage_metrics(metrics);
    register_proc_metrics(metrics);
    #[cfg(feature = "jemalloc")]
    jemalloc::register_metrics(metrics);
}

fn register_uptime_metric(metrics: &MetricRegistry) {
    let start = Instant::now();
    metrics.gauge("process.uptime", move || start.elapsed().as_micros() as u64);
}

fn register_panic_metric(metrics: &MetricRegistry) {
    let panics = metrics.counter("process.panics");
    let hook = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        panics.inc();
        hook(info)
    }));
}

fn register_rusage_metrics(metrics: &MetricRegistry) {
    metrics.gauge("process.user-time", || {
        Rusage::get_self().map_or(0, |r| r.user_time().as_micros() as u64)
    });
    metrics.gauge("process.user-time.norm", || {
        Rusage::get_self().map_or(0, |r| {
            r.user_time().as_micros() as u64 / num_cpus::get() as u64
        })
    });
    metrics.gauge("process.system-time", || {
        Rusage::get_self().map_or(0, |r| r.system_time().as_micros() as u64)
    });
    metrics.gauge("process.system-time.norm", || {
        Rusage::get_self().map_or(0, |r| {
            r.system_time().as_micros() as u64 / num_cpus::get() as u64
        })
    });
    metrics.gauge("process.blocks-read", || {
        Rusage::get_self().map_or(0, |r| r.blocks_read())
    });
    metrics.gauge("process.blocks-written", || {
        Rusage::get_self().map_or(0, |r| r.blocks_written())
    });
}

fn register_proc_metrics(metrics: &MetricRegistry) {
    if cfg!(not(target_os = "linux")) {
        info!("skipping /proc metric registration since this isn't Linux");
        return;
    }

    metrics.gauge("process.threads", || {
        pid::stat_self().map_or(0, |s| s.num_threads)
    });

    metrics.gauge("process.filedescriptor", || filedescriptor().unwrap_or(0.));
}

fn filedescriptor() -> io::Result<f32> {
    let mut files = 0;
    for r in fs::read_dir("/proc/self/fd")? {
        r?;
        files += 1;
    }

    let max_files = Rlimit::nofile()?.cur();

    Ok(files as f32 / max_files as f32)
}
