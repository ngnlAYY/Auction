//! Public/Admin HTTP listener 配置。

use serde::Deserialize;

/// HTTP 监听器允许使用的部署模式。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HttpListenMode {
    /// 监听 TCP host:port。
    #[default]
    Tcp,
    /// 监听 Linux Unix socket。
    Unix,
}

impl HttpListenMode {
    /// 返回稳定的配置/日志名称。
    pub const fn name(self) -> &'static str {
        match self {
            Self::Tcp => "tcp",
            Self::Unix => "unix",
        }
    }
}

/// Public/Admin HTTP 监听器的非敏感配置。
///
/// TCP 模式只使用 `address`；Unix 模式要求 `runtime_directory`、`socket_path` 和 `group`
/// 同时提供。socket 路径必须直接位于 runtime 目录下，实际绑定时还会再次检查目录、锁、
/// owner/group、权限和 socket 身份。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HttpListenerConfig {
    /// TCP 监听地址；省略时 Public/Admin 分别使用各自的 loopback 默认端口。
    pub address: String,
    /// Unix socket 所属用户组；TCP 模式下必须为空。
    pub group: String,
    /// 监听模式；可填 `tcp` 或 Linux 上的 `unix`，省略时为 `tcp`。
    pub mode: HttpListenMode,
    /// Unix socket 运行目录；TCP 模式下必须为空。
    pub runtime_directory: String,
    /// Unix socket 路径；TCP 模式下必须为空。
    pub socket_path: String,
}

impl HttpListenerConfig {
    /// 创建一个 TCP 监听配置。
    pub fn tcp(address: impl Into<String>) -> Self {
        Self { address: address.into(), ..Self::default() }
    }
}

impl Default for HttpListenerConfig {
    fn default() -> Self {
        Self {
            address: String::new(),
            group: String::new(),
            mode: HttpListenMode::Tcp,
            runtime_directory: String::new(),
            socket_path: String::new(),
        }
    }
}
