//! PostgreSQL 首次部署的独立、一次性 loopback HTTP 表面。

#[cfg(feature = "postgres")]
mod server;

#[cfg(feature = "postgres")]
pub use server::{FirstRunServerOptions, run_first_run};
