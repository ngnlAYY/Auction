//! Public HTTP 监听器装配；TCP/Unix 绑定与运行由 `auction-http` 共享实现。

use anyhow::Context;
use auction_config::{AppConfig, HttpListenMode};
use auction_http::HttpListener;
#[cfg(target_os = "linux")]
use auction_http::UnixListenerOptions;
use tokio::net::TcpListener;

/// 按已解析的 Public listener 配置绑定 TCP 或 Unix socket。
pub(crate) async fn bind(config: &AppConfig) -> anyhow::Result<HttpListener> {
    let listener_config =
        config.listener.as_ref().context("public-api must have an HTTP listener configuration")?;
    match listener_config.mode {
        HttpListenMode::Tcp => {
            let address = config.listen_addr.context("public-api must have a listen address")?;
            Ok(HttpListener::Tcp(TcpListener::bind(address).await?))
        }
        HttpListenMode::Unix => {
            #[cfg(target_os = "linux")]
            {
                let listener = auction_http::UnixHttpListener::bind(UnixListenerOptions {
                    runtime_directory: listener_config.runtime_directory.clone().into(),
                    socket_path: listener_config.socket_path.clone().into(),
                    group: listener_config.group.clone(),
                    lock_file_name: ".public-api-listener.lock",
                })
                .await
                .context("bind Public Unix listener")?;
                Ok(HttpListener::Unix(listener))
            }
            #[cfg(not(target_os = "linux"))]
            {
                anyhow::bail!("Public Unix listener 只支持 Linux")
            }
        }
    }
}
