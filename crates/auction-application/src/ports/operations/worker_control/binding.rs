//! Worker control socket 绑定的安全规则；配置加载、Worker 运行期与 Admin/CLI 客户端共用同一份
//! 校验，避免各处约束各改各的（例如 socket 长度上限只改一处）。本模块只做纯判定，不访问文件系统。

use std::path::{Component, Path, PathBuf};

use crate::ports::admin::valid_listener_group;

/// socket 路径的字节上限；为 `sockaddr_un.sun_path` 保留余量。
const MAX_SOCKET_PATH_BYTES: usize = 100;
/// peer UID 与 GID allowlist 合计的项数上限。
const MAX_PEER_ALLOWLIST_ENTRIES: usize = 64;

/// 绑定规则的失败类别闭集，按判定顺序排列；调用方映射为自己的错误类型与文案。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerControlBindingError {
    /// runtime 目录不是规范化的非根绝对路径。
    InvalidRuntimeDirectory,
    /// socket 路径不是规范化的非根绝对路径，或超过 100 字节。
    InvalidSocketPath,
    /// socket 不直接位于 runtime 目录。
    SocketOutsideRuntimeDirectory,
    /// group 不符合 Unix 监听 socket 属组名规则（与 Admin 监听属组共用同一判定）。
    InvalidGroup,
    /// peer UID 与 GID allowlist 合计超过 64 项。
    AllowlistTooLarge,
    /// 同一 allowlist 内有重复 ID。
    DuplicateAllowlistId,
    /// 要求显式 allowlist 时 UID 与 GID 都为空。
    EmptyAllowlist,
}

/// 校验 Worker control socket 绑定；`require_allowlist` 为 false 只用于 control 关闭时的配置
/// 校验，运行期绑定必须传 true，不能以空 allowlist 放行任意本机进程。
pub fn validate_control_binding(
    socket_path: &Path,
    runtime_directory: &Path,
    group: &str,
    allowed_peer_uids: &[u32],
    allowed_peer_gids: &[u32],
    require_allowlist: bool,
) -> Result<(), WorkerControlBindingError> {
    if !normalized_absolute_path(runtime_directory) {
        return Err(WorkerControlBindingError::InvalidRuntimeDirectory);
    }
    if !valid_control_socket_path(socket_path) {
        return Err(WorkerControlBindingError::InvalidSocketPath);
    }
    if socket_path.parent() != Some(runtime_directory) {
        return Err(WorkerControlBindingError::SocketOutsideRuntimeDirectory);
    }
    // 属组名规则只有一个 owner：与 Admin/Public 监听 socket 的属组名共用同一判定，不另写一份。
    if !valid_listener_group(group) {
        return Err(WorkerControlBindingError::InvalidGroup);
    }
    if allowed_peer_uids.len() + allowed_peer_gids.len() > MAX_PEER_ALLOWLIST_ENTRIES {
        return Err(WorkerControlBindingError::AllowlistTooLarge);
    }
    if has_duplicate(allowed_peer_uids) || has_duplicate(allowed_peer_gids) {
        return Err(WorkerControlBindingError::DuplicateAllowlistId);
    }
    if require_allowlist && allowed_peer_uids.is_empty() && allowed_peer_gids.is_empty() {
        return Err(WorkerControlBindingError::EmptyAllowlist);
    }
    Ok(())
}

/// socket 路径规则：规范化的非根绝对路径，且不超过 100 字节。客户端与 Worker 绑定共用，
/// 不接受服务端不会绑定的路径。
pub fn valid_control_socket_path(path: &Path) -> bool {
    normalized_absolute_path(path) && path.as_os_str().len() <= MAX_SOCKET_PATH_BYTES
}

/// 规范化的非根绝对路径：不含 `.`、`..`、重复分隔符或 NUL。
fn normalized_absolute_path(path: &Path) -> bool {
    let normalized: PathBuf = path.components().collect();
    path.is_absolute()
        && path != Path::new("/")
        && normalized.as_os_str() == path.as_os_str()
        && !path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
        && !path.to_string_lossy().chars().any(|character| character == '\0')
}

fn has_duplicate(values: &[u32]) -> bool {
    let mut values = values.to_vec();
    values.sort_unstable();
    values.windows(2).any(|pair| pair[0] == pair[1])
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{WorkerControlBindingError, valid_control_socket_path, validate_control_binding};
    use crate::ports::admin::valid_listener_group;

    const RUNTIME: &str = "/run/hv-auction/worker";
    const SOCKET: &str = "/run/hv-auction/worker/control.sock";

    fn check(
        socket: &str,
        runtime: &str,
        group: &str,
        uids: &[u32],
        gids: &[u32],
        require_allowlist: bool,
    ) -> Result<(), WorkerControlBindingError> {
        validate_control_binding(
            Path::new(socket),
            Path::new(runtime),
            group,
            uids,
            gids,
            require_allowlist,
        )
    }

    #[test]
    fn accepts_socket_directly_under_runtime_directory() {
        assert_eq!(check(SOCKET, RUNTIME, "hv-auction", &[1000], &[], true), Ok(()));
        assert_eq!(check(SOCKET, RUNTIME, "a.b_c-1", &[], &[2000, 2001], true), Ok(()));
        // control 关闭时的配置校验允许空 allowlist。
        assert_eq!(check(SOCKET, RUNTIME, "hv-auction", &[], &[], false), Ok(()));
    }

    #[test]
    fn rejects_paths_that_are_not_normalized_absolute() {
        for runtime in ["/", "run/hv-auction", "/run/./hv-auction", "/run/../hv", "/run//hv"] {
            assert_eq!(
                check("/run/hv/control.sock", runtime, "hv", &[1], &[], true),
                Err(WorkerControlBindingError::InvalidRuntimeDirectory),
                "{runtime}"
            );
        }
        for socket in ["control.sock", "/run/hv-auction/worker/../control.sock", "/run/a\0b"] {
            assert_eq!(
                check(socket, RUNTIME, "hv", &[1], &[], true),
                Err(WorkerControlBindingError::InvalidSocketPath),
                "{socket:?}"
            );
        }
        // runtime 目录先于 socket 判定。
        assert_eq!(
            check("control.sock", "/", "hv", &[1], &[], true),
            Err(WorkerControlBindingError::InvalidRuntimeDirectory)
        );
    }

    #[test]
    fn socket_path_is_limited_to_one_hundred_bytes() {
        let runtime = format!("/{}", "r".repeat(80));
        let at_limit = format!("{runtime}/{}", "s".repeat(100 - runtime.len() - 1));
        assert_eq!(at_limit.len(), 100);
        assert_eq!(check(&at_limit, &runtime, "hv", &[1], &[], true), Ok(()));
        let over_limit = format!("{at_limit}x");
        assert_eq!(
            check(&over_limit, &runtime, "hv", &[1], &[], true),
            Err(WorkerControlBindingError::InvalidSocketPath)
        );
    }

    /// 客户端只按 socket 路径规则校验：与绑定同口径，不要求知道 runtime 目录。
    #[test]
    fn client_socket_path_rule_matches_binding_rule() {
        assert!(valid_control_socket_path(Path::new(SOCKET)));
        let over_limit = format!("/{}", "s".repeat(100));
        for socket in [
            "control.sock",
            "/",
            "/run//hv/control.sock",
            "/run/hv/control.sock/",
            "/run/hv/../control.sock",
            over_limit.as_str(),
        ] {
            assert!(!valid_control_socket_path(Path::new(socket)), "{socket}");
            assert_eq!(
                check(socket, RUNTIME, "hv", &[1], &[], true),
                Err(WorkerControlBindingError::InvalidSocketPath),
                "{socket}"
            );
        }
    }

    #[test]
    fn socket_must_live_directly_in_runtime_directory() {
        for socket in ["/run/hv-auction/control.sock", "/run/hv-auction/worker/nested/control.sock"]
        {
            assert_eq!(
                check(socket, RUNTIME, "hv", &[1], &[], true),
                Err(WorkerControlBindingError::SocketOutsideRuntimeDirectory),
                "{socket}"
            );
        }
    }

    /// 属组名只有一个规则 owner：同一张边界表同时驱动 control 绑定与 Admin/Public 监听属组校验，
    /// 两者对每个取值的判定必须一致。
    #[test]
    fn group_rule_is_shared_with_listener_groups() {
        let at_limit = "g".repeat(64);
        let too_long = "g".repeat(65);
        let cases = [
            ("hv-auction", true),
            ("a.b_c-1", true),
            ("1000", true),
            ("0hv", true),
            (at_limit.as_str(), true),
            ("", false),
            (too_long.as_str(), false),
            ("组", false),
            ("hé", false),
            ("hv auction", false),
            ("hv/auction", false),
            ("-hv", false),
            (".hv", false),
            ("_hv", false),
        ];
        for (group, accepted) in cases {
            assert_eq!(valid_listener_group(group), accepted, "监听属组 {group:?}");
            let expected = accepted.ok_or(WorkerControlBindingError::InvalidGroup);
            assert_eq!(
                check(SOCKET, RUNTIME, group, &[1], &[], true),
                expected,
                "control 属组 {group:?}"
            );
        }
    }

    #[test]
    fn allowlist_is_bounded_unique_and_required_when_enabled() {
        let uids: Vec<u32> = (0..40).collect();
        let gids: Vec<u32> = (0..24).collect();
        assert_eq!(check(SOCKET, RUNTIME, "hv", &uids, &gids, true), Ok(()));
        let gids: Vec<u32> = (0..25).collect();
        assert_eq!(
            check(SOCKET, RUNTIME, "hv", &uids, &gids, true),
            Err(WorkerControlBindingError::AllowlistTooLarge)
        );
        // UID 与 GID 各自去重；跨列表相同的数值不算重复。
        assert_eq!(check(SOCKET, RUNTIME, "hv", &[7], &[7], true), Ok(()));
        for (uids, gids) in [(&[7, 7][..], &[][..]), (&[][..], &[3, 1, 3][..])] {
            assert_eq!(
                check(SOCKET, RUNTIME, "hv", uids, gids, false),
                Err(WorkerControlBindingError::DuplicateAllowlistId)
            );
        }
        assert_eq!(
            check(SOCKET, RUNTIME, "hv", &[], &[], true),
            Err(WorkerControlBindingError::EmptyAllowlist)
        );
    }
}
