//! tracing telemetry 子模块装配与稳定导出。

mod config;
mod health;
mod processor;
mod runtime;

pub use config::{InitError, ShutdownError, TracingConfig};
pub use runtime::{
    TracingRuntime, attach_remote_trace_context, init_json_stderr, init_with_config,
};
