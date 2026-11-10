//! Admin API 的装配入口。

mod config;
mod first_run;
mod listener;
mod service;
mod state;

/// 从显式角色路径启动，不解析 supervisor 的 CLI 或自动进入初始化。
pub async fn run_from_path(path: std::path::PathBuf) -> anyhow::Result<()> {
    service::run(Some(path)).await
}

/// 启动 Admin API，具体配置、状态装配和生命周期由对应 owner 持有。
pub async fn run() -> anyhow::Result<()> {
    match first_run::select_mode()? {
        first_run::StartMode::Normal(path) => service::run(path).await,
        first_run::StartMode::Setup(options) => first_run::run(options).await,
    }
}
