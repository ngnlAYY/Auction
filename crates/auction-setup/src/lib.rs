//! PostgreSQL 首次部署的独立一次性 HTTP 表面；默认 loopback，远程访问需显式代理授权。

#[cfg(feature = "postgres")]
mod server;

#[cfg(feature = "postgres")]
pub use server::{
    FirstRunOutcome, FirstRunPolicy, FirstRunServerOptions, RemoteSetupAccess, run_first_run,
    run_first_run_with_policy,
};
