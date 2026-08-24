//! Admin API 的装配入口。

mod config;
mod listener;
mod service;
mod state;

/// 启动 Admin API，具体配置、状态装配和生命周期由对应 owner 持有。
pub async fn run() -> anyhow::Result<()> {
    service::run().await
}
