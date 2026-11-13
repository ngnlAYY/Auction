//! 角色监听端点的推导与冲突判定；Admin runtime 覆盖后的 listener 由配置 crate 给出。

use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
};

use anyhow::Context;
use auction_config::{AppConfig, HttpListenMode};
use serde::Serialize;

/// 应用 readiness 实际可达的端点。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "address", rename_all = "snake_case")]
pub(crate) enum Endpoint {
    Tcp(SocketAddr),
    Unix(PathBuf),
}

impl Endpoint {
    pub(super) fn public(config: &AppConfig) -> anyhow::Result<Self> {
        let listener = config.listener.as_ref().context("Public 缺少 HTTP listener")?;
        match listener.mode {
            HttpListenMode::Tcp => {
                Ok(Self::Tcp(config.listen_addr.context("Public TCP listener 地址缺失")?))
            }
            HttpListenMode::Unix => Self::unix(&listener.socket_path, "Public Unix listener"),
        }
    }

    pub(super) fn admin(config: &AppConfig) -> anyhow::Result<Self> {
        if let Some(path) = config.admin_unix_listener_socket_path() {
            return Self::unix(path, "Admin Unix listener");
        }
        Ok(Self::Tcp(config.listen_addr.context("Admin 缺少 TCP 或 Unix listener")?))
    }

    pub(super) fn worker_control(config: &AppConfig) -> anyhow::Result<Option<Self>> {
        let control = &config.worker.control;
        if !control.enabled {
            return Ok(None);
        }
        Self::unix(&control.socket_path, "Worker control socket").map(Some)
    }

    fn unix(path: &str, kind: &str) -> anyhow::Result<Self> {
        let path = Path::new(path);
        if !path.is_absolute() {
            anyhow::bail!("{kind} 必须使用绝对路径");
        }
        Ok(Self::Unix(path.to_owned()))
    }

    /// 共享 readiness 探针的借用视图。
    pub(crate) fn readiness_target(&self) -> auction_process::ReadinessTarget<'_> {
        match self {
            Self::Tcp(address) => auction_process::ReadinessTarget::Tcp(*address),
            Self::Unix(path) => auction_process::ReadinessTarget::Unix(path),
        }
    }

    /// 同端口且地址相同或任一方为通配地址即冲突；Unix socket 以路径判定。
    pub(super) fn conflicts_with(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Tcp(left), Self::Tcp(right)) => {
                left.port() == right.port()
                    && (left.ip() == right.ip()
                        || left.ip().is_unspecified()
                        || right.ip().is_unspecified())
            }
            (Self::Unix(left), Self::Unix(right)) => left == right,
            _ => false,
        }
    }
}
