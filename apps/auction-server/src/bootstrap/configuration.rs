//! 单主机三角色部署的只读预检；加载仍复用配置层与驱动目标身份。

use super::{cli::ChildRole, filesystem};
use anyhow::{Context, bail};
use auction_config::{AppConfig, HttpListenMode};
use std::{
    fs,
    net::SocketAddr,
    os::unix::fs::{FileTypeExt, MetadataExt},
    path::{Path, PathBuf},
    time::Duration,
};

pub(super) struct RolePlan {
    pub role: ChildRole,
    pub path: PathBuf,
    pub config: AppConfig,
    pub endpoint: Endpoint,
}
#[derive(Clone, PartialEq, Eq)]
pub(super) enum Endpoint {
    Tcp(SocketAddr),
    Unix(PathBuf),
}

pub(super) fn load(directory: &Path) -> anyhow::Result<Vec<RolePlan>> {
    filesystem::trusted_directory(directory)?;
    let mut plans = Vec::new();
    for role in ChildRole::ORDER {
        let path = directory.join(format!("{}.toml", role.name()));
        regular(&path, false)?;
        let config =
            AppConfig::load_from_path(role.config_role(), &path).context("load deployment role")?;
        for path in [config.common_config_path.as_deref(), config.runtime_config_path.as_deref()]
            .into_iter()
            .flatten()
        {
            regular(path, false)?;
        }
        regular(config.secrets_path.as_deref().context("secrets_file required")?, true)?;
        if auction_storage::DatabaseBackend::from_url(
            config.database_url().context("database required")?,
        )? != auction_storage::DatabaseBackend::Postgres
        {
            bail!("single-binary deployment only supports PostgreSQL");
        }
        let endpoint = if role == ChildRole::Worker {
            if !config.worker.control.enabled {
                bail!("single-binary deployment requires Worker control");
            }
            // Supervisor readiness 使用当前 UID，不假设存在系统范围的专用用户组。
            if !config.worker.control.allowed_peer_uids.contains(&unsafe { libc::geteuid() }) {
                bail!("Worker control must allow the supervisor UID");
            }
            filesystem::trusted_directory(Path::new(&config.worker.control.runtime_directory))?;
            Endpoint::Unix(PathBuf::from(&config.worker.control.socket_path))
        } else if role == ChildRole::Admin {
            if let Some(path) = config.admin_unix_listener_socket_path() {
                Endpoint::Unix(path.into())
            } else {
                Endpoint::Tcp(config.listen_addr.context("Admin listener required")?)
            }
        } else {
            let listener = config.listener.as_ref().context("Public listener required")?;
            match listener.mode {
                HttpListenMode::Tcp => {
                    Endpoint::Tcp(config.listen_addr.context("Public TCP listener required")?)
                }
                HttpListenMode::Unix => Endpoint::Unix(PathBuf::from(&listener.socket_path)),
            }
        };
        match &endpoint {
            Endpoint::Tcp(address) if address.port() == 0 => {
                bail!("dynamic deployment ports are not supported")
            }
            Endpoint::Unix(path) if !path.is_absolute() => {
                bail!("deployment sockets must be absolute")
            }
            _ => (),
        }
        plans.push(RolePlan { role, path, config, endpoint });
    }
    let worker = &plans[0].config;
    let admin = &plans[1].config;
    if let Some(path) = &admin.worker_config_path {
        let worker_path = worker.config_path.as_deref().context("Worker 主配置路径缺失")?;
        if fs::canonicalize(path).context("读取 Admin worker_config_file 身份失败")?
            != fs::canonicalize(worker_path).context("读取 Worker 主配置身份失败")?
        {
            bail!("Admin worker_config_file 必须指向当前部署的 Worker 主配置");
        }
    }
    let expected = auction_storage::strict_deployment_database_identity(
        worker.database_url().context("database required")?,
    )?;
    let common =
        worker.common_config_path.as_ref().context("common_file required")?.canonicalize()?;
    let runtime = worker.runtime_config_path.as_ref().map(fs::canonicalize).transpose()?;
    for plan in &plans {
        if plan.config.runtime_config_path.as_ref().map(fs::canonicalize).transpose()? != runtime
            || plan.config.equipment_colors != worker.equipment_colors
        {
            bail!("三角色必须共用同一 runtime_config_file（或全部省略），且装备规则快照一致");
        }
        // 全部省略仍保留能力缺失语义；装备信封的三角色消费者不得持有不同密钥。
        if plan.config.equipment_data_key_id() != worker.equipment_data_key_id()
            || plan.config.equipment_data_key() != worker.equipment_data_key()
        {
            bail!("Public、Admin 与 Worker 必须使用相同的 equipment_data_key 标识和材料");
        }
        if auction_storage::strict_deployment_database_identity(
            plan.config.database_url().context("database required")?,
        )? != expected
            || plan
                .config
                .common_config_path
                .as_ref()
                .context("common_file required")?
                .canonicalize()?
                != common
        {
            bail!("all roles must share a database target and common_file");
        }
    }
    let admin_bot_players =
        admin.admin_runtime.as_ref().map(|runtime| runtime.bot_players.as_slice()).unwrap_or(&[]);
    if plans[2].config.bot_players != worker.bot_players || worker.bot_players != admin_bot_players
    {
        bail!("Public、Admin 与 Worker 加载的 bot_players 名单快照不一致");
    }
    if admin.auction != worker.auction
        || admin.worker_control_socket_path() != worker.worker.control.socket_path
        || admin.user_login_data_key_id() != worker.user_login_data_key_id()
        || admin.user_login_data_key() != worker.user_login_data_key()
        || admin.credential_data_key_id() != worker.credential_data_key_id()
        || admin.credential_data_key() != worker.credential_data_key()
    {
        bail!("Admin/Worker runtime, policy, control or data-key topology mismatch");
    }
    for (index, left) in plans.iter().enumerate() {
        for right in &plans[index + 1..] {
            let conflict = match (&left.endpoint, &right.endpoint) {
                (Endpoint::Tcp(left), Endpoint::Tcp(right)) => {
                    left.port() == right.port()
                        && (left.ip() == right.ip()
                            || left.ip().is_unspecified()
                            || right.ip().is_unspecified())
                }
                (Endpoint::Unix(left), Endpoint::Unix(right)) => left == right,
                _ => false,
            };
            if conflict {
                bail!("role listeners must not overlap");
            }
        }
    }
    Ok(plans)
}

fn regular(path: &Path, secret: bool) -> anyhow::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.mode() & if secret { 0o077 } else { 0o022 } != 0
    {
        bail!("unsafe deployment configuration file: {}", path.display());
    }
    let parent = path.parent().context("config file parent required")?;
    filesystem::trusted_directory(parent)
}

pub(super) async fn ensure_available(plans: &[RolePlan]) -> anyhow::Result<()> {
    for plan in plans {
        match &plan.endpoint {
            Endpoint::Tcp(address) => {
                let _listener = tokio::net::TcpListener::bind(address)
                    .await
                    .context("role listener already in use")?;
            }
            Endpoint::Unix(path) => {
                let metadata = match fs::symlink_metadata(path) {
                    Ok(metadata) => metadata,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(error) => return Err(error).context("inspect role socket"),
                };
                // 与角色绑定器保持一致：只允许当前服务账号拥有的 socket 进入 stale 检查。
                // SAFETY：geteuid 只读取当前进程的有效 UID。
                if !metadata.file_type().is_socket() || metadata.uid() != unsafe { libc::geteuid() }
                {
                    bail!("unsafe role socket path: {}", path.display());
                }
                match tokio::time::timeout(
                    Duration::from_millis(100),
                    tokio::net::UnixStream::connect(path),
                )
                .await
                {
                    Ok(Ok(_stream)) => bail!("role socket is already active: {}", path.display()),
                    Ok(Err(error))
                        if matches!(
                            error.kind(),
                            std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound
                        ) =>
                    {
                        // 连接失败后重新核对 inode；路径已消失可以继续，替换则失败关闭。
                        match fs::symlink_metadata(path) {
                            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                            Err(error) => return Err(error).context("reinspect role socket"),
                            Ok(current)
                                if current.file_type().is_socket()
                                    && current.uid() == metadata.uid()
                                    && current.dev() == metadata.dev()
                                    && current.ino() == metadata.ino() => {}
                            Ok(_) => {
                                bail!("role socket changed during preflight: {}", path.display())
                            }
                        }
                    }
                    Ok(Err(error)) => return Err(error).context("cannot verify role socket"),
                    Err(_) => bail!("role socket probe timed out: {}", path.display()),
                }
                // 不清理任何路径；stale socket 仍由对应角色的安全 listener owner 处理。
            }
        }
    }
    Ok(())
}
