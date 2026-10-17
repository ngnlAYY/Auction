//! tracing telemetry 子模块装配与稳定导出。

mod config;
mod health;
#[path = "processor/mod.rs"]
mod processor;
mod runtime;

pub use config::{InitError, ShutdownError, TracingConfig};
pub use runtime::{TracingRuntime, attach_remote_trace_context, init, init_with_config};
