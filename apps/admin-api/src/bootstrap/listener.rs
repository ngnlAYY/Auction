//! Admin HTTP 监听器装配。
//!
//! TCP 与 Unix socket 共用同一 Axum 路由。Unix socket 的目录、单实例锁、stale socket、
//! owner/group、权限和身份校验由 `auction-http` 的共享绑定器统一负责；本模块只选择 Admin
//! runtime 的有效模式并把非敏感参数传入。

use std::{future::Future, io};

use anyhow::Context;
use auction_application::AdminRuntimeListenMode;
use auction_config::AppConfig;
#[cfg(target_os = "linux")]
use auction_http::{UnixHttpListener, UnixListenerOptions};
use axum::Router;
use tokio::net::TcpListener;

/// Admin HTTP 的实际监听器；Unix 变体只在 Linux 目标编译。
pub(crate) enum AdminListener {
    Tcp(TcpListener),
    #[cfg(target_os = "linux")]
    Unix(UnixHttpListener),
}

impl AdminListener {
    /// 返回不含敏感配置的监听描述，用于启动日志。
    pub(crate) fn description(&self) -> String {
        match self {
            Self::Tcp(listener) => listener
                .local_addr()
                .map(|address| address.to_string())
                .unwrap_or_else(|_| "tcp".to_owned()),
            #[cfg(target_os = "linux")]
            Self::Unix(listener) => format!("unix:{}", listener.socket_path().display()),
        }
    }
}

/// 按已解析的 Admin runtime 选择并绑定监听器。
pub(crate) async fn bind(config: &AppConfig) -> anyhow::Result<AdminListener> {
    let mode = config
        .admin_runtime
        .as_ref()
        .map(|runtime| runtime.listener.mode)
        .unwrap_or(AdminRuntimeListenMode::Tcp);
    match mode {
        AdminRuntimeListenMode::Tcp => {
            let address = config.listen_addr.context("admin-api must have a listen address")?;
            Ok(AdminListener::Tcp(TcpListener::bind(address).await?))
        }
        AdminRuntimeListenMode::Unix => {
            #[cfg(target_os = "linux")]
            {
                let runtime = config
                    .admin_runtime
                    .as_ref()
                    .context("Unix listener requires an Admin runtime document")?;
                let listener = UnixHttpListener::bind(UnixListenerOptions {
                    runtime_directory: runtime.listener.runtime_directory.clone().into(),
                    socket_path: runtime.listener.socket_path.clone().into(),
                    group: runtime.listener.group.clone(),
                    lock_file_name: ".admin-api-listener.lock",
                })
                .await
                .context("bind Admin Unix listener")?;
                Ok(AdminListener::Unix(listener))
            }
            #[cfg(not(target_os = "linux"))]
            {
                anyhow::bail!("Admin Unix listener 只支持 Linux")
            }
        }
    }
}

/// 使用统一的 `SocketAddr` connect-info 运行 TCP 或 Unix 路由。
pub(crate) async fn serve(
    listener: AdminListener,
    app: Router,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> io::Result<()> {
    match listener {
        AdminListener::Tcp(listener) => auction_http::serve_http(listener, app, shutdown).await,
        #[cfg(target_os = "linux")]
        AdminListener::Unix(listener) => auction_http::serve_http(listener, app, shutdown).await,
    }
}
