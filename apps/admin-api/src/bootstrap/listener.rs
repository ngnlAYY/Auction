//! Admin HTTP 监听器装配。
//!
//! TCP 与 Unix socket 共用同一 Axum 路由。Unix 模式只在 Linux 上启用，并在绑定前检查
//! runtime 目录、单实例锁、socket 身份、owner/group 和权限；这样 stale socket 不会覆盖仍在
//! 使用中的 Admin 进程，也不会让 runtime 文档中的 Unix 字段停留在“可解析但不生效”。

use std::{future::Future, io, net::SocketAddr};

#[cfg(target_os = "linux")]
use std::{
    ffi::CString,
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::Context;
use auction_application::AdminRuntimeListenMode;
use auction_config::AppConfig;
use axum::{Extension, Router, extract::ConnectInfo};
use tokio::net::TcpListener;

/// Admin HTTP 的实际监听器；Unix 变体只在 Linux 目标编译。
pub(crate) enum AdminListener {
    Tcp(TcpListener),
    #[cfg(target_os = "linux")]
    Unix(AdminUnixListener),
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
            Self::Unix(listener) => format!("unix:{}", listener.socket_path.display()),
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
                Ok(AdminListener::Unix(AdminUnixListener::bind(&runtime.listener).await?))
            }
            #[cfg(not(target_os = "linux"))]
            {
                anyhow::bail!("Admin Unix listener 只支持 Linux")
            }
        }
    }
}

/// 使用统一的 `SocketAddr` connect-info 运行 TCP 或 Unix 路由。
///
/// Unix 请求没有网络地址，监听器会为其注入 loopback 占位地址；HTTP 安全策略仍由 socket
/// 文件的 owner/group/权限和上层 origin/CSRF 校验共同保护。
pub(crate) async fn serve(
    listener: AdminListener,
    app: Router,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> io::Result<()> {
    match listener {
        AdminListener::Tcp(listener) => {
            axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
                .with_graceful_shutdown(shutdown)
                .await
        }
        #[cfg(target_os = "linux")]
        AdminListener::Unix(listener) => {
            let app = app.layer(Extension(ConnectInfo(UNIX_LISTENER_ADDRESS)));
            axum::serve(listener, app.into_make_service()).with_graceful_shutdown(shutdown).await
        }
    }
}

#[cfg(target_os = "linux")]
const UNIX_LISTENER_ADDRESS: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 0);

#[cfg(target_os = "linux")]
#[derive(Clone, Copy, PartialEq, Eq)]
struct FileIdentity {
    device: u64,
    inode: u64,
}

#[cfg(target_os = "linux")]
pub(crate) struct AdminUnixListener {
    listener: tokio::net::UnixListener,
    socket_path: PathBuf,
    identity: FileIdentity,
    _lock: File,
}

#[cfg(target_os = "linux")]
impl AdminUnixListener {
    async fn bind(
        config: &auction_application::AdminRuntimeListenerConfig,
    ) -> anyhow::Result<Self> {
        use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};

        let runtime_directory = Path::new(&config.runtime_directory);
        let socket_path = Path::new(&config.socket_path);
        let group_id = resolve_group_id(&config.group)?;
        validate_runtime_directory(runtime_directory, group_id)?;
        let lock = acquire_lock(runtime_directory)?;
        prepare_socket_path(socket_path).await?;

        let unix_listener = tokio::net::UnixListener::bind(socket_path)?;
        let identity = (|| -> anyhow::Result<FileIdentity> {
            let identity = socket_identity(socket_path)?;
            set_socket_security(socket_path, group_id)?;
            let metadata = fs::symlink_metadata(socket_path)?;
            if !metadata.file_type().is_socket()
                || metadata.uid() != current_uid()
                || metadata.gid() != group_id
                || metadata.permissions().mode() & 0o777 != 0o660
                || socket_identity(socket_path)? != identity
            {
                anyhow::bail!("Admin Unix socket owner/group/权限或身份校验失败");
            }
            Ok(identity)
        })();
        let identity = match identity {
            Ok(identity) => identity,
            Err(error) => {
                drop(unix_listener);
                let _ = fs::remove_file(socket_path);
                return Err(error);
            }
        };

        Ok(Self {
            listener: unix_listener,
            socket_path: socket_path.to_owned(),
            identity,
            _lock: lock,
        })
    }
}

#[cfg(target_os = "linux")]
impl axum::serve::Listener for AdminUnixListener {
    type Io = tokio::net::UnixStream;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            match self.listener.accept().await {
                Ok((stream, _address)) => return (stream, UNIX_LISTENER_ADDRESS),
                Err(error) => {
                    tracing::error!(error = %error, "Admin Unix listener accept 失败");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        Ok(UNIX_LISTENER_ADDRESS)
    }
}

#[cfg(target_os = "linux")]
impl Drop for AdminUnixListener {
    fn drop(&mut self) {
        if socket_identity(&self.socket_path).is_ok_and(|identity| identity == self.identity) {
            let _ = fs::remove_file(&self.socket_path);
        }
    }
}

#[cfg(target_os = "linux")]
fn current_uid() -> u32 {
    // SAFETY: geteuid 无需指针或外部内存，始终返回当前进程的有效 UID。
    unsafe { libc::geteuid() }
}

#[cfg(target_os = "linux")]
fn resolve_group_id(group: &str) -> anyhow::Result<u32> {
    if let Ok(group_id) = group.parse::<u32>() {
        return Ok(group_id);
    }
    let name = CString::new(group).context("Unix socket group 名称包含非法字符")?;
    // SAFETY: name 是 NUL 结尾且只在本次启动同步读取 libc 返回的静态 group 记录。
    let record = unsafe { libc::getgrnam(name.as_ptr()) };
    if record.is_null() {
        anyhow::bail!("无法解析 Admin Unix socket group")
    }
    // SAFETY: getgrnam 返回的非空指针指向 libc 保证有效的 group 记录。
    Ok(unsafe { (*record).gr_gid })
}

#[cfg(target_os = "linux")]
fn validate_runtime_directory(path: &Path, group_id: u32) -> anyhow::Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let metadata =
        fs::symlink_metadata(path).context("读取 Admin Unix listener runtime 目录失败")?;
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || metadata.uid() != current_uid()
        || metadata.permissions().mode() & 0o002 != 0
        || metadata.permissions().mode() & 0o020 != 0 && metadata.gid() != group_id
    {
        anyhow::bail!("Admin Unix listener runtime 目录不满足 owner/group/权限约束")
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn acquire_lock(runtime_directory: &Path) -> anyhow::Result<File> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
    use std::os::unix::io::AsRawFd;

    let lock_path = runtime_directory.join(".admin-api-listener.lock");
    let lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(&lock_path)
        .context("打开 Admin Unix listener 单实例锁失败")?;
    lock.set_permissions(fs::Permissions::from_mode(0o600))?;
    let metadata = lock.metadata()?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != current_uid()
        || metadata.permissions().mode() & 0o777 != 0o600
    {
        anyhow::bail!("Admin Unix listener 单实例锁不满足安全约束")
    }
    // SAFETY: fd 来自仍由当前函数持有的 regular file；flock 不修改 Rust 管理的内存。
    let result = unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result != 0 {
        let error = io::Error::last_os_error();
        let raw_error = error.raw_os_error();
        if raw_error == Some(libc::EWOULDBLOCK) || raw_error == Some(libc::EAGAIN) {
            anyhow::bail!("Admin Unix listener 已由其他进程占用")
        }
        return Err(error).context("获取 Admin Unix listener 单实例锁失败");
    }
    Ok(lock)
}

#[cfg(target_os = "linux")]
async fn prepare_socket_path(path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};

    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).context("读取 Admin Unix listener socket 失败"),
    };
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_socket()
        || metadata.uid() != current_uid()
    {
        anyhow::bail!("Admin Unix listener socket 路径已被非安全对象占用")
    }
    match tokio::time::timeout(Duration::from_millis(100), tokio::net::UnixStream::connect(path))
        .await
    {
        Ok(Ok(stream)) => {
            drop(stream);
            anyhow::bail!("Admin Unix listener 已由其他进程占用")
        }
        Ok(Err(error))
            if matches!(
                error.kind(),
                io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound
            ) =>
        {
            match fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(error).context("删除已失效的 Admin Unix listener socket 失败");
                }
            }
            Ok(())
        }
        Ok(Err(error)) => Err(error).context("无法确认 Admin Unix listener socket 是否已失效"),
        Err(_) => anyhow::bail!("无法在限定时间内确认 Admin Unix listener socket 是否已失效"),
    }
}

#[cfg(target_os = "linux")]
fn socket_identity(path: &Path) -> anyhow::Result<FileIdentity> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};

    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_socket()
        || metadata.uid() != current_uid()
    {
        anyhow::bail!("Admin Unix listener socket 身份无效")
    }
    Ok(FileIdentity { device: metadata.dev(), inode: metadata.ino() })
}

#[cfg(target_os = "linux")]
fn set_socket_security(path: &Path, group_id: u32) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let encoded = CString::new(path.to_string_lossy().as_bytes())?;
    // SAFETY: encoded 是已校验路径的 NUL 结尾副本；geteuid/gid 使用当前进程和已解析组。
    let result = unsafe { libc::chown(encoded.as_ptr(), libc::geteuid(), group_id) };
    if result != 0 {
        return Err(io::Error::last_os_error())
            .context("设置 Admin Unix listener owner/group 失败");
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o660))
        .context("设置 Admin Unix listener 权限失败")?;
    Ok(())
}
