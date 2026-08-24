use super::*;

pub(crate) fn validate_worker_control(config: &WorkerControlConfig) -> Result<(), ConfigError> {
    let socket_path = Path::new(&config.socket_path);
    let runtime_directory = Path::new(&config.runtime_directory);
    if !valid_worker_control_path(runtime_directory, None) {
        return Err(ConfigError::InvalidWorkerControlConfig(
            "runtime_directory 必须是规范化的非根绝对路径",
        ));
    }
    if !valid_worker_control_path(socket_path, Some(100)) {
        return Err(ConfigError::InvalidWorkerControlConfig(
            "socket_path 必须是规范化绝对路径且长度不超过 100 字节",
        ));
    }
    if socket_path.parent() != Some(runtime_directory) {
        return Err(ConfigError::InvalidWorkerControlConfig(
            "socket_path 的直接父目录必须等于 runtime_directory",
        ));
    }
    if !valid_safe_identifier(&config.group) {
        return Err(ConfigError::InvalidWorkerControlConfig("group 必须是 1 至 64 位安全标识"));
    }
    if config.allowed_peer_uids.len() + config.allowed_peer_gids.len() > 64 {
        return Err(ConfigError::InvalidWorkerControlConfig("peer allowlist 不能超过 64 项"));
    }
    if duplicate_u32(&config.allowed_peer_uids) || duplicate_u32(&config.allowed_peer_gids) {
        return Err(ConfigError::InvalidWorkerControlConfig("peer allowlist 不能包含重复 ID"));
    }
    if config.enabled && config.allowed_peer_uids.is_empty() && config.allowed_peer_gids.is_empty()
    {
        return Err(ConfigError::InvalidWorkerControlConfig(
            "启用 Worker control 时必须显式配置 peer UID 或 GID allowlist",
        ));
    }
    Ok(())
}

pub(crate) fn valid_worker_control_path(path: &Path, maximum_bytes: Option<usize>) -> bool {
    let normalized: PathBuf = path.components().collect();
    path.is_absolute()
        && path != Path::new("/")
        && normalized.as_os_str() == path.as_os_str()
        && !path.components().any(|component| {
            matches!(component, std::path::Component::CurDir | std::path::Component::ParentDir)
        })
        && !path.as_os_str().to_string_lossy().chars().any(|character| character == '\0')
        && maximum_bytes.is_none_or(|maximum| path.as_os_str().len() <= maximum)
}

pub(crate) fn valid_safe_identifier(value: &str) -> bool {
    let mut bytes = value.bytes();
    matches!(bytes.next(), Some(first) if first.is_ascii_alphanumeric())
        && value.len() <= 64
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

pub(crate) fn duplicate_u32(values: &[u32]) -> bool {
    let mut values = values.to_vec();
    values.sort_unstable();
    values.windows(2).any(|pair| pair[0] == pair[1])
}
