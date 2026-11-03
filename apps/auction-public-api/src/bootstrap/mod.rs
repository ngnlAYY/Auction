//! Public API 的装配入口。

mod config;
mod listener;
mod metrics;
mod service;
mod state;

pub use service::run_from_path;

/// 启动 Public API，具体配置、状态装配和生命周期由对应 owner 持有。
pub async fn run() -> anyhow::Result<()> {
    service::run().await
}
