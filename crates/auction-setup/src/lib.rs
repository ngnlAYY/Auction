//! PostgreSQL 首次部署的独立一次性 HTTP 表面；默认 loopback，远程访问需显式代理授权。

// 测试里的断言直接解包；该豁免只在测试构建生效，生产构建仍按 workspace lint 拒绝 unwrap/expect。
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod server;

pub use server::{
    FirstRunOutcome, FirstRunPolicy, FirstRunServerOptions, RemoteSetupAccess, run_first_run,
    run_first_run_with_policy,
};
