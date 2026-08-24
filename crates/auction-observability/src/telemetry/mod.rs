//! tracing telemetry 子模块装配与稳定导出。
//!
//! 配置、健康状态、批处理器和进程运行时分别由子模块持有；本文件不再承载实现细节。

mod config;
mod health;
mod processor;
mod runtime;

pub use config::{InitError, ShutdownError, TracingConfig};
pub use runtime::{TracingRuntime, attach_remote_trace_context, init, init_with_config};
