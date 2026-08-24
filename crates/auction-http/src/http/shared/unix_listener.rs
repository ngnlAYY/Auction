//! Public/Admin 共用的 Linux Unix HTTP listener。
//!
//! Unix socket 不是“把 TCP 地址换成路径”这么简单：绑定前必须确认 runtime 目录、单实例锁、
//! stale socket、owner/group、权限和最终 socket 身份，退出时也只能删除本进程实际创建的 socket。
//! 这组约束由 HTTP 适配器统一实现，Public 与 Admin 只提供各自的运行目录、socket 和锁文件名。

#![cfg(target_os = "linux")]

use std::{
    ffi::CString,
    fs::{self, File, OpenOptions},
    io,
    net::SocketAddr,
    os::unix::{
        fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
        io::AsRawFd,
    },
    path::{Path, PathBuf},
    time::Duration,
};

use axum::serve::Listener;

const UNIX_LISTENER_ADDRESS: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 0);

/// Unix HTTP listener 的绑定参数。
///
/// `lock_file_name` 由装配根使用固定常量提供，不能来自运行时请求或用户输入；Public 与 Admin
/// 即使共享 runtime 目录，也必须使用不同的锁文件名。
#[derive(Debug)]
pub struct UnixListenerOptions {
    /// 预先建立且由运行账号管理的 runtime 目录。
    pub runtime_directory: PathBuf,
    /// 直接位于 runtime 目录下的 socket 路径。
    pub socket_path: PathBuf,
    /// socket 所属的 Unix group 名称或数字 GID。
    pub group: String,
    /// 运行账号专用的单实例锁文件名。
    pub lock_file_name: &'static str,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct FileIdentity {
    device: u64,
    inode: u64,
}

/// 已完成安全绑定的 Unix HTTP listener。
pub struct UnixHttpListener {
    listener: tokio::net::UnixListener,
    socket_path: PathBuf,
    identity: FileIdentity,
    _lock: File,
}

impl UnixHttpListener {
    /// 校验并绑定 Unix socket；stale 目标只在确认是当前运行账号创建的 socket 后清理。
    pub async fn bind(options: UnixListenerOptions) -> io::Result<Self> {
        if options.socket_path.parent() != Some(options.runtime_directory.as_path()) {
            return Err(invalid("Unix socket 必须直接位于 runtime_directory 下"));
        }
        let group_id = resolve_group_id(&options.group)?;
        validate_runtime_directory(&options.runtime_directory, group_id)?;
        let lock = acquire_instance_lock(&options.runtime_directory, options.lock_file_name)?;
        prepare_socket_path(&options.socket_path).await?;

        let listener = tokio::net::UnixListener::bind(&options.socket_path)?;
        let identity = socket_identity(&options.socket_path)?;
        if let Err(error) = secure_bound_socket(&options.socket_path, group_id, identity) {
            drop(listener);
            let _ = remove_if_same_socket(&options.socket_path, Some(identity));
            return Err(error);
        }

        Ok(Self { listener, socket_path: options.socket_path, identity, _lock: lock })
    }

    /// 返回安全绑定后的 socket 路径，用于不含凭据的启动日志和部署描述。
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }
}

impl Listener for UnixHttpListener {
    type Io = tokio::net::UnixStream;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            match self.listener.accept().await {
                Ok((stream, _address)) => return (stream, UNIX_LISTENER_ADDRESS),
                Err(error) => {
                    tracing::error!(error = %error, "Unix HTTP listener accept 失败");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        Ok(UNIX_LISTENER_ADDRESS)
    }
}

impl Drop for UnixHttpListener {
    fn drop(&mut self) {
        let _ = remove_if_same_socket(&self.socket_path, Some(self.identity));
    }
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

fn with_context(error: io::Error, context: &str) -> io::Error {
    io::Error::new(error.kind(), format!("{context}: {error}"))
}

fn current_uid() -> u32 {
    // SAFETY: geteuid 无需指针或外部内存，始终返回当前进程的有效 UID。
    unsafe { libc::geteuid() }
}

fn resolve_group_id(group: &str) -> io::Result<u32> {
    if let Ok(group_id) = group.parse::<u32>() {
        return Ok(group_id);
    }
    let name = CString::new(group).map_err(|_| invalid("Unix socket group 名称包含非法字符"))?;
    // SAFETY: name 是 NUL 结尾且只在本次启动同步读取 libc 返回的静态 group 记录。
    let record = unsafe { libc::getgrnam(name.as_ptr()) };
    if record.is_null() {
        return Err(invalid("无法解析 Unix socket group"));
    }
    // SAFETY: getgrnam 返回的非空指针指向 libc 保证有效的 group 记录。
    Ok(unsafe { (*record).gr_gid })
}

fn validate_runtime_directory(path: &Path, group_id: u32) -> io::Result<()> {
    let resolved = fs::canonicalize(path)
        .map_err(|error| with_context(error, "规范化 Unix listener runtime 目录失败"))?;
    if resolved != path {
        return Err(invalid("Unix listener runtime 目录不能通过符号链接访问"));
    }
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| with_context(error, "读取 Unix listener runtime 目录失败"))?;
    let mode = metadata.permissions().mode();
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || metadata.uid() != current_uid()
        || mode & 0o002 != 0
        || mode & 0o020 != 0 && metadata.gid() != group_id
    {
        return Err(invalid("Unix listener runtime 目录不满足 owner/group/权限约束"));
    }
    Ok(())
}

fn acquire_instance_lock(runtime_directory: &Path, lock_file_name: &str) -> io::Result<File> {
    if lock_file_name.is_empty()
        || Path::new(lock_file_name).file_name().and_then(|name| name.to_str())
            != Some(lock_file_name)
    {
        return Err(invalid("Unix listener lock 文件名无效"));
    }
    let lock_path = runtime_directory.join(lock_file_name);
    let lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(&lock_path)
        .map_err(|error| with_context(error, "打开 Unix listener 单实例锁失败"))?;
    lock.set_permissions(fs::Permissions::from_mode(0o600))
        .map_err(|error| with_context(error, "设置 Unix listener 单实例锁权限失败"))?;
    let metadata =
        lock.metadata().map_err(|error| with_context(error, "读取 Unix listener 单实例锁失败"))?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != current_uid()
        || metadata.permissions().mode() & 0o777 != 0o600
    {
        return Err(invalid("Unix listener 单实例锁不满足安全约束"));
    }
    // SAFETY: fd 来自仍由当前函数持有的 regular file；flock 不修改 Rust 管理的内存。
    let result = unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result != 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::EWOULDBLOCK)
            || error.raw_os_error() == Some(libc::EAGAIN)
        {
            return Err(invalid("Unix listener 已由其他进程占用"));
        }
        return Err(with_context(error, "获取 Unix listener 单实例锁失败"));
    }
    Ok(lock)
}

async fn prepare_socket_path(path: &Path) -> io::Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(with_context(error, "读取 Unix listener socket 失败")),
    };
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_socket()
        || metadata.uid() != current_uid()
    {
        return Err(invalid("Unix listener socket 路径已被非安全对象占用"));
    }
    let identity = FileIdentity { device: metadata.dev(), inode: metadata.ino() };
    match tokio::time::timeout(Duration::from_millis(100), tokio::net::UnixStream::connect(path))
        .await
    {
        Ok(Ok(stream)) => {
            drop(stream);
            Err(invalid("Unix listener 已由其他进程占用"))
        }
        Ok(Err(error))
            if matches!(
                error.kind(),
                io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound
            ) =>
        {
            remove_if_same_socket(path, Some(identity))
        }
        Ok(Err(error)) => Err(with_context(error, "无法确认 Unix listener socket 是否已失效")),
        Err(_) => Err(invalid("无法在限定时间内确认 Unix listener socket 是否已失效")),
    }
}

fn socket_identity(path: &Path) -> io::Result<FileIdentity> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| with_context(error, "读取 Unix listener socket 身份失败"))?;
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_socket()
        || metadata.uid() != current_uid()
    {
        return Err(invalid("Unix listener socket 身份无效"));
    }
    Ok(FileIdentity { device: metadata.dev(), inode: metadata.ino() })
}

fn secure_bound_socket(path: &Path, group_id: u32, identity: FileIdentity) -> io::Result<()> {
    let encoded = CString::new(path.to_string_lossy().as_bytes())
        .map_err(|_| invalid("Unix listener socket 路径包含非法字符"))?;
    // SAFETY: encoded 是已校验路径的 NUL 结尾副本；geteuid/gid 使用当前进程和已解析组。
    let result = unsafe { libc::chown(encoded.as_ptr(), libc::geteuid(), group_id) };
    if result != 0 {
        return Err(with_context(
            io::Error::last_os_error(),
            "设置 Unix listener owner/group 失败",
        ));
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o660))
        .map_err(|error| with_context(error, "设置 Unix listener 权限失败"))?;
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| with_context(error, "读取 Unix listener socket 安全属性失败"))?;
    if !metadata.file_type().is_socket()
        || metadata.uid() != current_uid()
        || metadata.gid() != group_id
        || metadata.permissions().mode() & 0o777 != 0o660
        || socket_identity(path)? != identity
    {
        return Err(invalid("Unix listener socket owner/group/权限或身份校验失败"));
    }
    Ok(())
}

fn remove_if_same_socket(path: &Path, expected: Option<FileIdentity>) -> io::Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(with_context(error, "读取 Unix listener socket 失败")),
    };
    if metadata.file_type().is_symlink() || !metadata.file_type().is_socket() {
        return Err(invalid("拒绝删除非 socket 的 Unix listener 路径"));
    }
    let identity = FileIdentity { device: metadata.dev(), inode: metadata.ino() };
    if expected.is_some_and(|expected| expected != identity) {
        return Err(invalid("Unix listener socket 身份已变化，拒绝删除"));
    }
    fs::remove_file(path)
        .map_err(|error| with_context(error, "删除 stale Unix listener socket 失败"))
}
