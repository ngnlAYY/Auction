//! 三进程部署配置、listener 拓扑和权限预检。

use super::*;

struct DeploymentConfig {
    role: Role,
    config: AppConfig,
    backend: DatabaseBackend,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum DeploymentListener {
    None,
    Tcp(SocketAddr),
    Unix(PathBuf),
}

impl DeploymentListener {
    fn description(&self) -> String {
        match self {
            Self::None => "none".to_owned(),
            Self::Tcp(address) => format!("tcp:{address}"),
            Self::Unix(path) => format!("unix:{}", path.display()),
        }
    }
}

/// 执行不连接数据库、不启动进程的三进程部署预检。
///
/// 旧版启动脚本同时承担配置检查、进程装配和数据库操作；该只读命令保持独立，也由
/// `deployment start/restart` 在任何 unit mutation 前复用，避免预检触发外部 MoogleMail/论坛请求。
pub(super) fn deployment_check(config_dir: &Path, strict_permissions: bool) -> anyhow::Result<()> {
    let config_dir_metadata = fs::symlink_metadata(config_dir).with_context(|| {
        format!("read deployment config directory metadata: {}", config_dir.display())
    })?;
    if !config_dir_metadata.is_dir() || config_dir_metadata.file_type().is_symlink() {
        anyhow::bail!("deployment config directory is not a directory: {}", config_dir.display());
    }
    validate_non_secret_permissions(config_dir, "deployment config directory", strict_permissions)?;

    let mut configs = Vec::with_capacity(3);
    for (role, filename) in
        [(Role::Public, "public.toml"), (Role::Admin, "admin.toml"), (Role::Worker, "worker.toml")]
    {
        let path = config_dir.join(filename);
        require_regular_file(&path, "主配置")?;
        validate_non_secret_permissions(&path, "主配置", strict_permissions)?;
        let config = AppConfig::load_from_path(role, &path)
            .with_context(|| format!("load {} configuration", role.name()))?;
        let backend = config
            .database_url()
            .map(DatabaseBackend::from_url)
            .transpose()
            .with_context(|| format!("validate {} database_url", role.name()))?
            .with_context(|| format!("{} database_url is required", role.name()))?;
        if let Some(secrets_path) = config.secrets_path.as_deref() {
            require_regular_file(secrets_path, "敏感配置")?;
            report_secret_permissions(secrets_path, strict_permissions)?;
        } else {
            anyhow::bail!("{} configuration does not declare secrets_file", role.name());
        }
        if let Some(runtime_path) = config.runtime_config_path.as_deref() {
            require_regular_file(runtime_path, "runtime 配置")?;
            validate_non_secret_permissions(runtime_path, "runtime 配置", strict_permissions)?;
        }
        if role == Role::Worker && config.worker.control.enabled {
            validate_absolute_path(&config.worker.control.socket_path, "Worker control socket")?;
        }
        let listener = match role {
            Role::Public => public_deployment_listener(&config)?,
            Role::Admin => admin_deployment_listener(&config)?,
            Role::Worker => DeploymentListener::None,
        };
        println!(
            "ok role={} config={} database_backend={} listener={}",
            role.name(),
            path.display(),
            backend,
            listener.description(),
        );
        configs.push(DeploymentConfig { role, config, backend });
    }

    let expected_backend = configs[0].backend;
    if configs.iter().any(|entry| entry.backend != expected_backend) {
        anyhow::bail!(
            "deployment database backend mismatch; all public/admin/worker configs must use {}",
            expected_backend
        );
    }

    let public = configs
        .iter()
        .find(|entry| entry.role == Role::Public)
        .ok_or_else(|| anyhow::anyhow!("public config was not loaded"))?;
    let admin = configs
        .iter()
        .find(|entry| entry.role == Role::Admin)
        .ok_or_else(|| anyhow::anyhow!("admin config was not loaded"))?;
    let worker = configs
        .iter()
        .find(|entry| entry.role == Role::Worker)
        .ok_or_else(|| anyhow::anyhow!("worker config was not loaded"))?;
    validate_admin_worker_topology(&admin.config, &worker.config)?;

    let public_listener = public_deployment_listener(&public.config)?;
    let admin_listener = admin_deployment_listener(&admin.config)?;
    if let DeploymentListener::Tcp(address) = &public_listener
        && address.port() == 0
    {
        anyhow::bail!("public listener cannot use dynamic port 0");
    }
    if let DeploymentListener::Tcp(address) = &admin_listener
        && address.port() == 0
    {
        anyhow::bail!("admin listener cannot use dynamic port 0");
    }
    if deployment_listeners_conflict(&public_listener, &admin_listener) {
        anyhow::bail!("public and admin listener endpoints must be different");
    }

    println!(
        "deployment-check passed roles=3 database_backend={} strict_permissions={}",
        expected_backend, strict_permissions
    );
    Ok(())
}

pub(super) fn public_deployment_listener(config: &AppConfig) -> anyhow::Result<DeploymentListener> {
    let listener =
        config.listener.as_ref().ok_or_else(|| anyhow::anyhow!("public must expose a listener"))?;
    match listener.mode {
        HttpListenMode::Tcp => config
            .listen_addr
            .map(DeploymentListener::Tcp)
            .ok_or_else(|| anyhow::anyhow!("public TCP listener address is missing")),
        HttpListenMode::Unix => {
            validate_absolute_path(&listener.socket_path, "Public Unix listener socket")?;
            Ok(DeploymentListener::Unix(PathBuf::from(&listener.socket_path)))
        }
    }
}

pub(super) fn admin_deployment_listener(config: &AppConfig) -> anyhow::Result<DeploymentListener> {
    if let Some(socket_path) = config.admin_unix_listener_socket_path() {
        validate_absolute_path(socket_path, "Admin Unix listener socket")?;
        return Ok(DeploymentListener::Unix(PathBuf::from(socket_path)));
    }
    config
        .listen_addr
        .map(DeploymentListener::Tcp)
        .ok_or_else(|| anyhow::anyhow!("admin must expose a TCP or Unix listener"))
}

fn deployment_listeners_conflict(left: &DeploymentListener, right: &DeploymentListener) -> bool {
    match (left, right) {
        (DeploymentListener::None, _) | (_, DeploymentListener::None) => false,
        (DeploymentListener::Tcp(left), DeploymentListener::Tcp(right)) => {
            left.port() == right.port()
                && (left.ip() == right.ip()
                    || left.ip().is_unspecified()
                    || right.ip().is_unspecified())
        }
        (DeploymentListener::Unix(left), DeploymentListener::Unix(right)) => left == right,
        (DeploymentListener::Tcp(_), DeploymentListener::Unix(_))
        | (DeploymentListener::Unix(_), DeploymentListener::Tcp(_)) => false,
    }
}

fn validate_admin_worker_topology(admin: &AppConfig, worker: &AppConfig) -> anyhow::Result<()> {
    if admin.worker_control_socket_path() != worker.worker.control.socket_path.as_str() {
        anyhow::bail!("Admin 与 Worker 必须配置相同的 Worker control socket_path");
    }
    if admin
        .admin_unix_listener_socket_path()
        .is_some_and(|path| path == worker.worker.control.socket_path.as_str())
    {
        anyhow::bail!("Admin HTTP Unix listener 不能复用 Worker control socket_path");
    }
    for (name, admin_id, admin_key, worker_id, worker_key) in [
        (
            "equipment_data_key",
            admin.equipment_data_key_id(),
            admin.equipment_data_key(),
            worker.equipment_data_key_id(),
            worker.equipment_data_key(),
        ),
        (
            "user_login_data_key",
            admin.user_login_data_key_id(),
            admin.user_login_data_key(),
            worker.user_login_data_key_id(),
            worker.user_login_data_key(),
        ),
        (
            "credential_data_key",
            admin.credential_data_key_id(),
            admin.credential_data_key(),
            worker.credential_data_key_id(),
            worker.credential_data_key(),
        ),
    ] {
        if admin_id != worker_id || admin_key != worker_key {
            anyhow::bail!("Admin 与 Worker 必须使用相同的 {name} 标识和材料");
        }
    }
    Ok(())
}

fn require_regular_file(path: &Path, kind: &str) -> anyhow::Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("read {kind} metadata: {}", path.display()))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        anyhow::bail!("{kind} is not an ordinary non-symlink file: {}", path.display());
    }
    Ok(())
}

fn validate_non_secret_permissions(path: &Path, kind: &str, strict: bool) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        let mode = fs::symlink_metadata(path)?.permissions().mode();
        if mode & 0o022 != 0 {
            if strict {
                anyhow::bail!("{kind} 不得允许 group/other 写入: {}", path.display());
            }
            println!(
                "warning deployment_file_permissions={} kind={} mode={:o}; use --strict-permissions for deployment gating",
                path.display(),
                kind,
                mode & 0o777,
            );
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (path, kind, strict);
    }
    Ok(())
}

fn validate_absolute_path(value: &str, kind: &str) -> anyhow::Result<()> {
    if value.trim().is_empty() || !Path::new(value).is_absolute() {
        anyhow::bail!("{kind} path must be absolute");
    }
    Ok(())
}

fn report_secret_permissions(path: &Path, strict: bool) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        let mode = fs::symlink_metadata(path)?.permissions().mode();
        if mode & 0o077 != 0 {
            if strict {
                anyhow::bail!("敏感配置权限过宽: {} (需要去除 group/other 权限)", path.display());
            }
            println!(
                "warning sensitive_config_permissions={} mode={:o}; use --strict-permissions for deployment gating",
                path.display(),
                mode & 0o777
            );
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (path, strict);
        println!("warning sensitive_config_permissions=unsupported-platform");
    }
    Ok(())
}
