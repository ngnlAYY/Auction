//! Worker control 配置的边界校验；绑定安全规则与 Worker 运行期共用
//! [`validate_control_binding`]，这里只把失败类别映射回既有配置错误文案。

use std::path::Path;

use auction_application::operations::{WorkerControlBindingError, validate_control_binding};

use super::super::{ConfigError, WorkerControlConfig};

pub(crate) fn validate_worker_control(config: &WorkerControlConfig) -> Result<(), ConfigError> {
    // 关闭 control 时仍校验路径与身份形状，只是不强制 allowlist 非空。
    validate_control_binding(
        Path::new(&config.socket_path),
        Path::new(&config.runtime_directory),
        &config.group,
        &config.allowed_peer_uids,
        &config.allowed_peer_gids,
        config.enabled,
    )
    .map_err(|error| ConfigError::InvalidWorkerControlConfig(binding_message(error)))
}

/// 配置错误文案是运维可见契约，逐字保持。
const fn binding_message(error: WorkerControlBindingError) -> &'static str {
    match error {
        WorkerControlBindingError::InvalidRuntimeDirectory => {
            "runtime_directory 必须是规范化的非根绝对路径"
        }
        WorkerControlBindingError::InvalidSocketPath => {
            "socket_path 必须是规范化绝对路径且长度不超过 100 字节"
        }
        WorkerControlBindingError::SocketOutsideRuntimeDirectory => {
            "socket_path 的直接父目录必须等于 runtime_directory"
        }
        WorkerControlBindingError::InvalidGroup => "group 必须是 1 至 64 位安全标识",
        WorkerControlBindingError::AllowlistTooLarge => "peer allowlist 不能超过 64 项",
        WorkerControlBindingError::DuplicateAllowlistId => "peer allowlist 不能包含重复 ID",
        WorkerControlBindingError::EmptyAllowlist => {
            "启用 Worker control 时必须显式配置 peer UID 或 GID allowlist"
        }
    }
}
