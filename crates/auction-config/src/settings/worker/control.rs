//! Worker control Unix socket 配置。

use serde::Deserialize;

/// Worker control Unix socket 的非敏感配置。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WorkerControlConfig {
    /// 是否启用 Worker control；省略时关闭。
    pub enabled: bool,
    /// Unix socket 所属组名；省略时使用旧版默认组 `auction-worker-control`。
    pub group: String,
    pub allowed_peer_gids: Vec<u32>,
    pub allowed_peer_uids: Vec<u32>,
    /// 预先建立的规范化绝对运行目录；省略时使用 `/run/hv-auction/worker`。
    pub runtime_directory: String,
    /// 规范化绝对 socket 路径；省略时使用 `/run/hv-auction/worker/control.sock`。
    pub socket_path: String,
}

impl Default for WorkerControlConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            group: "auction-worker-control".to_owned(),
            allowed_peer_gids: Vec::new(),
            allowed_peer_uids: Vec::new(),
            runtime_directory: "/run/hv-auction/worker".to_owned(),
            socket_path: "/run/hv-auction/worker/control.sock".to_owned(),
        }
    }
}
