//! Public HTTP 监听器装配；TCP/Unix 绑定复用共享安全实现，本模块只传入 listener
//! 选择与固定锁文件名。

use std::{future::Future, io};

use anyhow::Context;
use auction_config::{AppConfig, HttpListenMode};
#[cfg(target_os = "linux")]
use auction_http::{UnixHttpListener, UnixListenerOptions};
use axum::Router;
use tokio::net::TcpListener;

/// Public HTTP 的实际监听器；Unix 变体只在 Linux 目标编译。
pub(crate) enum PublicListener {
    Tcp(TcpListener),
    #[cfg(target_os = "linux")]
    Unix(UnixHttpListener),
}

impl PublicListener {
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

/// 按已解析的 Public listener 配置绑定 TCP 或 Unix socket。
pub(crate) async fn bind(config: &AppConfig) -> anyhow::Result<PublicListener> {
    let listener_config =
        config.listener.as_ref().context("public-api must have an HTTP listener configuration")?;
    match listener_config.mode {
        HttpListenMode::Tcp => {
            let address = config.listen_addr.context("public-api must have a listen address")?;
            Ok(PublicListener::Tcp(TcpListener::bind(address).await?))
        }
        HttpListenMode::Unix => {
            #[cfg(target_os = "linux")]
            {
                let listener = UnixHttpListener::bind(UnixListenerOptions {
                    runtime_directory: listener_config.runtime_directory.clone().into(),
                    socket_path: listener_config.socket_path.clone().into(),
                    group: listener_config.group.clone(),
                    lock_file_name: ".public-api-listener.lock",
                })
                .await
                .context("bind Public Unix listener")?;
                Ok(PublicListener::Unix(listener))
            }
            #[cfg(not(target_os = "linux"))]
            {
                anyhow::bail!("Public Unix listener 只支持 Linux")
            }
        }
    }
}

/// 使用统一的 `SocketAddr` connect-info 运行 TCP 或 Unix 路由。
pub(crate) async fn serve(
    listener: PublicListener,
    app: Router,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> io::Result<()> {
    match listener {
        PublicListener::Tcp(listener) => auction_http::serve_http(listener, app, shutdown).await,
        #[cfg(target_os = "linux")]
        PublicListener::Unix(listener) => auction_http::serve_http(listener, app, shutdown).await,
    }
}
