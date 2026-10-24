//! 三进程部署配置、listener 拓扑和权限预检。

use super::*;

struct DeploymentConfig {
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

/// 执行不连接数据库、不启动进程的三进程部署预检，按 Public、Admin、Worker 顺序返回快照。
/// 启动与迁移直接消费通过拓扑校验的同一组值，不重新读取一组未经联合校验的配置。
pub(super) fn deployment_check(
    config_dir: &Path,
    strict_permissions: bool,
) -> anyhow::Result<[AppConfig; 3]> {
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
        let common_path = config.common_config_path.as_deref().with_context(|| {
            format!("{} configuration does not declare common_file", role.name())
        })?;
        require_regular_file(common_path, "公共配置")?;
        validate_non_secret_permissions(common_path, "公共配置", strict_permissions)?;
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
            validate_runtime_permissions(runtime_path, strict_permissions)?;
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
            "ok role={} config={} common_config={} database_backend={} listener={}",
            role.name(),
            path.display(),
            common_path.display(),
            backend,
            listener.description(),
        );
        configs.push(DeploymentConfig { config, backend });
    }

    let [public, admin, worker]: [DeploymentConfig; 3] =
        configs.try_into().map_err(|_| anyhow::anyhow!("部署配置必须包含三个角色"))?;
    let expected_backend = public.backend;
    if admin.backend != expected_backend || worker.backend != expected_backend {
        anyhow::bail!(
            "deployment database backend mismatch; all public/admin/worker configs must use {}",
            expected_backend
        );
    }

    let expected_database = super::deployment_identity::database_identity(
        public.config.database_url().context("Public 数据库配置缺失")?,
    )?;
    for entry in [&admin, &worker] {
        let identity = super::deployment_identity::database_identity(
            entry.config.database_url().context("部署数据库配置缺失")?,
        )?;
        if identity != expected_database {
            anyhow::bail!("Public、Admin 与 Worker 必须指向同一个数据库目标");
        }
    }

    let common_path = public.config.common_config_path.as_deref();
    if common_path.is_none()
        || admin.config.common_config_path.as_deref() != common_path
        || worker.config.common_config_path.as_deref() != common_path
    {
        anyhow::bail!("Public、Admin 与 Worker 必须引用同一份 common_file");
    }
    validate_runtime_topology(&public.config, &admin.config, &worker.config)?;
    validate_equipment_key_topology(&public.config, &admin.config, &worker.config)?;
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
    if worker.config.worker.control.enabled
        && let DeploymentListener::Unix(path) = &public_listener
        && path.as_path() == Path::new(&worker.config.worker.control.socket_path)
    {
        anyhow::bail!("Public HTTP Unix listener 不能复用 Worker control socket_path");
    }

    println!(
        "deployment-check passed roles=3 database_backend={} strict_permissions={}",
        expected_backend, strict_permissions
    );
    Ok([public.config, admin.config, worker.config])
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

/// 装备信封由三角色共同消费；全部省略仍表示能力缺失，不在预检中强制启用。
fn validate_equipment_key_topology(
    public: &AppConfig,
    admin: &AppConfig,
    worker: &AppConfig,
) -> anyhow::Result<()> {
    for config in [public, admin] {
        if config.equipment_data_key_id() != worker.equipment_data_key_id()
            || config.equipment_data_key() != worker.equipment_data_key()
        {
            anyhow::bail!("Public、Admin 与 Worker 必须使用相同的 equipment_data_key 标识和材料");
        }
    }
    Ok(())
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
    if let Some(path) = &admin.worker_config_path {
        let worker_path = worker.config_path.as_deref().context("Worker 主配置路径缺失")?;
        if fs::canonicalize(path).context("读取 Admin worker_config_file 身份失败")?
            != fs::canonicalize(worker_path).context("读取 Worker 主配置身份失败")?
        {
            anyhow::bail!("Admin worker_config_file 必须指向当前部署的 Worker 主配置");
        }
    }
    for (name, admin_id, admin_key, worker_id, worker_key) in [
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

/// 装备分组与论坛标色共用启动规则；两类创建者继续使用一致的新场次模板。
fn validate_runtime_topology(
    public: &AppConfig,
    admin: &AppConfig,
    worker: &AppConfig,
) -> anyhow::Result<()> {
    let expected = public
        .runtime_config_path
        .as_deref()
        .map(fs::canonicalize)
        .transpose()
        .context("读取 Public runtime 文件身份失败")?;
    for config in [admin, worker] {
        let path = config
            .runtime_config_path
            .as_deref()
            .map(fs::canonicalize)
            .transpose()
            .context("读取部署 runtime 文件身份失败")?;
        if path != expected {
            anyhow::bail!("Public、Admin 与 Worker 必须引用同一份 runtime_config_file，或全部省略");
        }
        // 同一路径可能在三次加载之间被管理端替换；规则快照也必须一致。
        if config.equipment_colors != public.equipment_colors {
            anyhow::bail!("Public、Admin 与 Worker 加载的装备分组与论坛标色规则不一致");
        }
    }
    if admin.auction != worker.auction {
        anyhow::bail!("Admin 与 Worker 的有效拍卖默认策略或排期不一致");
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

/// runtime 存储要求精确 0640；部署者可能是 root，owner 必须另以实际 Admin unit 身份核对。
fn validate_runtime_permissions(path: &Path, strict: bool) -> anyhow::Result<()> {
    let parent = path.parent().context("runtime 配置父目录缺失")?;
    let parent_metadata = fs::symlink_metadata(parent).context("读取 runtime 配置父目录失败")?;
    if !parent_metadata.is_dir() || parent_metadata.file_type().is_symlink() {
        anyhow::bail!("runtime 配置父目录必须是非符号链接目录：{}", parent.display());
    }
    validate_non_secret_permissions(parent, "runtime 配置父目录", strict)?;
    #[cfg(unix)]
    if fs::symlink_metadata(path)?.permissions().mode() & 0o777 != 0o640 {
        if strict {
            anyhow::bail!("runtime 配置必须使用 0640 权限：{}", path.display());
        }
        println!("warning runtime 配置须使用 0640 权限：{}", path.display());
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
