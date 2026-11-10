//! Admin HTTP 监听器装配；socket 目录、锁、owner/group 与身份校验由 `auction-http`
//! 共享绑定器负责，本模块只选择 Admin 有效模式并传参。

use anyhow::Context;
use auction_config::AppConfig;
use auction_http::HttpListener;
#[cfg(target_os = "linux")]
use auction_http::UnixListenerOptions;
use tokio::net::TcpListener;

/// 按已解析的 Admin runtime 选择并绑定监听器。
pub(crate) async fn bind(config: &AppConfig) -> anyhow::Result<HttpListener> {
    let listener = config.listener.as_ref().context("admin-api must have a listener")?;
    match listener.mode {
        auction_config::HttpListenMode::Tcp => {
            let address = config.listen_addr.context("admin-api must have a listen address")?;
            Ok(HttpListener::Tcp(TcpListener::bind(address).await?))
        }
        auction_config::HttpListenMode::Unix => {
            #[cfg(target_os = "linux")]
            {
                let listener = auction_http::UnixHttpListener::bind(UnixListenerOptions {
                    runtime_directory: listener.runtime_directory.clone().into(),
                    socket_path: listener.socket_path.clone().into(),
                    group: listener.group.clone(),
                    lock_file_name: ".admin-api-listener.lock",
                })
                .await
                .context("bind Admin Unix listener")?;
                Ok(HttpListener::Unix(listener))
            }
            #[cfg(not(target_os = "linux"))]
            {
                anyhow::bail!("Admin Unix listener 只支持 Linux")
            }
        }
    }
}
