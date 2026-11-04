//! Public/Admin 共用的 Linux Unix HTTP listener。
//! Unix socket 不是“把 TCP 地址换成路径”这么简单：绑定前必须确认 runtime 目录、单实例锁、
//! stale socket、owner/group、权限和最终 socket 身份，退出时也只能删除本进程实际创建的 socket。
//! 这组约束由 HTTP 适配器统一实现，Public 与 Admin 只提供各自的运行目录、socket 和锁文件名。

#![cfg(target_os = "linux")]

use auction_application::diagnostics::DiagnosticContext;

use crate::diagnostics;

use std::{
    ffi::CString,
    fs::{self, File, OpenOptions},
    io,
    net::SocketAddr,
    os::unix::{
        ffi::OsStrExt,
        fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
        io::AsRawFd,
    },
    path::{Path, PathBuf},
    time::Duration,
};

use axum::serve::Listener;

/// Unix HTTP listener 的绑定参数。
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
        validate_runtime_directory(&options.runtime_directory)?;
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
    type Addr = Option<SocketAddr>;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            match self.listener.accept().await {
                Ok((stream, _address)) => return (stream, None),
                Err(error) => {
                    diagnostics::error(
                        "http_listener",
                        "accept_unix_connection",
                        diagnostics::io_kind(&error),
                        DiagnosticContext { route: Some("__listener__"), ..Default::default() },
                    );
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        Ok(None)
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
    // NSS 非重入接口返回进程共享记录；并发绑定不同 listener 时不能借用该静态内存。
    // 与 deployment control 的账号解析保持同一有界缓冲策略，超限或未知组均失败关闭。
    let mut buffer = vec![0_u8; 1024];
    loop {
        let mut record = std::mem::MaybeUninit::<libc::group>::uninit();
        let mut result = std::ptr::null_mut();
        // SAFETY: name 为 NUL 结尾，record、buffer 和 result 在调用期间有效且互不重叠。
        let status = unsafe {
            libc::getgrnam_r(
                name.as_ptr(),
                record.as_mut_ptr(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut result,
            )
        };
        if status == libc::ERANGE && buffer.len() < 1024 * 1024 {
            buffer.resize(buffer.len() * 2, 0);
            continue;
        }
        if status != 0 || result.is_null() {
            return Err(invalid("无法解析 Unix socket group"));
        }
        // SAFETY: 成功且 result 非空时，getgrnam_r 已完整初始化当前调用独占的 record。
        return Ok(unsafe { record.assume_init().gr_gid });
    }
}

fn validate_runtime_directory(path: &Path) -> io::Result<()> {
    let resolved = fs::canonicalize(path)
        .map_err(|error| with_context(error, "规范化 Unix listener runtime 目录失败"))?;
    if !path.is_absolute() || resolved != path {
        return Err(invalid("Unix listener runtime 目录必须是无符号链接的规范化绝对路径"));
    }
    let owner = current_uid();
    // 从根向下确认目录链不可被其他账号替换；仅保护末级目录仍会留下祖先重命名竞态。
    // 代理组只需遍历目录和连接 socket，不能拥有目录写权限；root 和运行账号属于信任边界。
    let ancestors = path.ancestors().collect::<Vec<_>>();
    for directory in ancestors.into_iter().rev() {
        let metadata = fs::symlink_metadata(directory)
            .map_err(|error| with_context(error, "读取 Unix listener 目录链失败"))?;
        let mode = metadata.permissions().mode();
        // /tmp 一类 root-owned sticky 祖先不能被其他账号用于替换后续已验证的目录。
        let protected_shared_ancestor =
            directory != path && metadata.uid() == 0 && mode & 0o1000 != 0;
        if !metadata.is_dir()
            || metadata.file_type().is_symlink()
            || mode & 0o022 != 0 && !protected_shared_ancestor
            || metadata.uid() != owner && (directory == path || metadata.uid() != 0)
        {
            return Err(invalid(
                "Unix listener 目录链必须由 root 或运行账号拥有且禁止 group/other 写入，runtime 目录必须由运行账号拥有",
            ));
        }
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
    // 先验证文件身份，再修改权限；拒绝硬链接或其他对象时不能连带修改原文件。
    let metadata =
        lock.metadata().map_err(|error| with_context(error, "读取 Unix listener 单实例锁失败"))?;
    if !metadata.is_file() || metadata.nlink() != 1 || metadata.uid() != current_uid() {
        return Err(invalid("Unix listener 单实例锁不满足安全约束"));
    }
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
        Err(cause) => {
            diagnostics::warn(
                "http_listener",
                "probe_stale_socket_timeout",
                diagnostics::timeout_kind(&cause),
                DiagnosticContext { route: Some("__listener__"), ..Default::default() },
            );
            Err(invalid("无法在限定时间内确认 Unix listener socket 是否已失效"))
        }
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
    // 目录链校验阻止非受信账号替换路径；副作用前还需拒绝已经变化的 socket 身份。
    if socket_identity(path)? != identity {
        return Err(invalid("Unix listener socket 身份已变化，拒绝修改权限"));
    }
    // Unix 路径可以包含非 UTF-8 字节；不能用显示替换字符把 chown 指向另一个文件。
    let encoded = CString::new(path.as_os_str().as_bytes())
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
