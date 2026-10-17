//! 单主机三角色部署的只读预检；加载仍复用配置层与驱动目标身份。

use super::{cli::ChildRole, filesystem};
use anyhow::{Context, bail};
use auction_config::{AppConfig, HttpListenMode};
use std::{
    fs,
    net::SocketAddr,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
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
    let expected = auction_storage::strict_deployment_database_identity(
        worker.database_url().context("database required")?,
    )?;
    let common =
        worker.common_config_path.as_ref().context("common_file required")?.canonicalize()?;
    for plan in &plans {
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
    if admin.auction != worker.auction
        || admin.runtime_config_path != worker.runtime_config_path
        || admin.worker_control_socket_path() != worker.worker.control.socket_path
        || admin.equipment_data_key_id() != worker.equipment_data_key_id()
        || admin.equipment_data_key() != worker.equipment_data_key()
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
                if tokio::net::UnixStream::connect(path).await.is_ok() {
                    bail!("role socket is already active: {}", path.display());
                }
                // 不清理任何路径；stale socket 仍由对应角色的安全 listener owner 处理。
            }
        }
    }
    Ok(())
}
