//! 应用配置文件定位、读取和输入模型装配。

use crate::config::reader::read_toml_file;

use super::super::*;
use super::model::{AppConfig, BuildInput, CommonFileConfig, FileConfig, PublicSessionFileConfig};

impl AppConfig {
    pub fn load(role: Role) -> Result<Self, ConfigError> {
        if let Some(path) = env::var_os(CONFIG_FILE_ENV).map(PathBuf::from) {
            return Self::load_from_path(role, path);
        }

        let default_path = PathBuf::from("ops/config").join(format!("{}.toml", role.name()));
        match fs::symlink_metadata(&default_path) {
            Ok(_) => return Self::load_from_path(role, default_path),
            Err(source) if source.kind() == io::ErrorKind::NotFound => {}
            Err(source) => return Err(ConfigError::Read { path: default_path, source }),
        }

        Self::build(
            role,
            BuildInput {
                config_path: None,
                common_config_path: None,
                listen_address: None,
                listener: None,
                log_filter: None,
                metrics: MetricsConfig::default(),
                readiness: ReadinessConfig::default(),
                database_profiles: DatabaseProfilesConfig::default(),
                database_roles: DatabaseRolesConfig::default(),
                database_pool: DatabasePoolConfig::default(),
                tracing: TracingSettings::default(),
                shutdown: ShutdownFileConfig::default(),
                realtime: RealtimeConfig::default(),
                backup: BackupConfig::default(),
                auction: AuctionConfig::default(),
                worker: WorkerConfig::default(),
                http_security: None,
                session: None,
                secrets_path: None,
                runtime_config_path: None,
                worker_config_path: None,
                runtime_worker_budget: None,
            },
        )
    }

    pub fn load_from_path(role: Role, path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        // 先固定角色文件的绝对位置，再解析其相对引用；不解析 symlink，保留 runtime
        // 存储对路径与文件类型的安全检查，避免默认发现路径在正式装配时被当成相对路径拒绝。
        let requested_path = path.as_ref();
        let path = std::path::absolute(requested_path)
            .map_err(|source| ConfigError::Read { path: requested_path.to_owned(), source })?;
        let contents = read_toml_file(&path)
            .map_err(|source| ConfigError::Read { path: path.clone(), source })?;
        reject_unsupported_role_sections(role, &contents)?;
        match role {
            Role::Public => {
                let mut file: FileConfig<PublicSessionFileConfig> = toml::from_str(&contents)
                    .map_err(|_| ConfigError::Parse { path: path.clone() })?;
                // 在加载公共文件或敏感文件前拒绝旧时限，错误只包含固定迁移说明。
                let session =
                    RoleSessionConfig::Public(file.session.take().unwrap_or_default().resolve()?);
                Self::from_file(role, Some(path), file, session)
            }
            Role::Admin | Role::Worker => {
                let mut file: FileConfig<SessionConfig> = toml::from_str(&contents)
                    .map_err(|_| ConfigError::Parse { path: path.clone() })?;
                let session = file
                    .session
                    .take()
                    .map(RoleSessionConfig::Timed)
                    .unwrap_or_else(|| RoleSessionConfig::for_role(role));
                Self::from_file(role, Some(path), file, session)
            }
        }
    }

    fn from_file<Session>(
        role: Role,
        config_path: Option<PathBuf>,
        file: FileConfig<Session>,
        session: RoleSessionConfig,
    ) -> Result<Self, ConfigError> {
        let FileConfig {
            common_file,
            database_pool,
            listen_address,
            listener,
            tracing,
            shutdown,
            realtime,
            secrets_file,
            runtime_config_file,
            worker_config_file,
            worker,
            http_security,
            session: _,
        } = file;
        if worker_config_file.is_some() && role != Role::Admin {
            return Err(ConfigError::InvalidRuntimeWorkerConfig(
                "worker_config_file 只能由 Admin 角色配置",
            ));
        }
        let common_config_path = resolve_config_path(config_path.as_deref(), common_file);
        let common = load_common_file(&common_config_path)?;
        let CommonFileConfig {
            log_filter,
            metrics,
            readiness,
            database_profiles,
            database_roles,
            backup,
            auction,
        } = common;
        let worker = resolve_worker_paths(config_path.as_deref(), worker);
        let secrets_path = Some(resolve_secrets_path(config_path.as_deref(), secrets_file));
        let runtime_config_path =
            runtime_config_file.map(|path| resolve_config_path(config_path.as_deref(), path));
        let worker_config_path =
            worker_config_file.map(|path| resolve_config_path(config_path.as_deref(), path));
        let runtime_worker_budget = worker_config_path
            .as_deref()
            .map(|path| load_runtime_worker_budget(path, runtime_config_path.as_deref()))
            .transpose()?;

        Self::build(
            role,
            BuildInput {
                config_path,
                common_config_path: Some(common_config_path),
                listen_address,
                listener,
                log_filter,
                metrics,
                readiness,
                database_profiles,
                database_roles,
                database_pool,
                tracing,
                shutdown,
                realtime,
                backup,
                auction,
                worker,
                http_security,
                session: Some(session),
                secrets_path,
                runtime_config_path,
                worker_config_path,
                runtime_worker_budget,
            },
        )
    }
}

/// 角色文件只接受该角色真正消费的区块。
///
/// 解析模型为三角色共用，因此不属于该角色的区块（如 Worker 的 `[listener]`）会被
/// 接受却永不生效。配置规则要求失败关闭：这里在读入后立即拒绝，避免保留“看起来生效”
/// 的配置，也避免回退到旧程序时由对端静默忽略。
fn reject_unsupported_role_sections(role: Role, contents: &str) -> Result<(), ConfigError> {
    let unsupported: &[&'static str] = match role {
        Role::Public => &["worker"],
        Role::Admin => &["realtime"],
        Role::Worker => &["listener", "http_security", "realtime", "session"],
    };
    // 语法错误交由后续 serde 解析报告，这里不重复解释同一份文件。
    let Ok(document) = toml::from_str::<toml::Value>(contents) else {
        return Ok(());
    };
    let Some(table) = document.as_table() else {
        return Ok(());
    };
    for section in unsupported {
        if table.contains_key(*section) {
            return Err(ConfigError::UnsupportedRoleSection { role: role.name(), section });
        }
    }
    // Admin 只消费空闲/绝对时限；公开会话上限由 Public 从自己的角色文件与共享 runtime 读取。
    if role == Role::Admin
        && table
            .get("session")
            .and_then(toml::Value::as_table)
            .is_some_and(|session| session.contains_key("max_public_sessions"))
    {
        return Err(ConfigError::AdminSessionLimitNotApplied);
    }
    Ok(())
}

/// 公共文件使用独立闭集模型；解析错误只报告路径，避免旧文件中的误放敏感值被回显。
fn load_common_file(path: &Path) -> Result<CommonFileConfig, ConfigError> {
    let contents = read_toml_file(path)
        .map_err(|source| ConfigError::Read { path: path.to_owned(), source })?;
    toml::from_str(&contents).map_err(|_| ConfigError::Parse { path: path.to_owned() })
}

/// 有界读取真实 Worker 角色文件并冻结最小预算基值，不递归加载 common 或 secrets。
fn load_runtime_worker_budget(
    path: &Path,
    runtime_path: Option<&Path>,
) -> Result<auction_application::admin::AdminRuntimeMailDeliveryBudget, ConfigError> {
    let runtime_path = runtime_path.ok_or(ConfigError::InvalidRuntimeWorkerConfig(
        "worker_config_file 要求 Admin 同时配置 runtime_config_file",
    ))?;
    let contents = read_toml_file(path)
        .map_err(|source| ConfigError::Read { path: path.to_owned(), source })?;
    let file: FileConfig<SessionConfig> =
        toml::from_str(&contents).map_err(|_| ConfigError::Parse { path: path.to_owned() })?;
    if file.worker_config_file.is_some() {
        return Err(ConfigError::InvalidRuntimeWorkerConfig(
            "worker_config_file 必须指向 Worker 角色文件，不能包含另一个 worker_config_file",
        ));
    }
    let worker_runtime_path =
        file.runtime_config_file.ok_or(ConfigError::InvalidRuntimeWorkerConfig(
            "worker_config_file 指向的 Worker 必须配置同源 runtime_config_file",
        ))?;
    let worker_runtime_path = resolve_config_path(Some(path), worker_runtime_path);
    let worker_runtime_identity = fs::canonicalize(&worker_runtime_path)
        .map_err(|source| ConfigError::Read { path: worker_runtime_path, source })?;
    let runtime_identity = fs::canonicalize(runtime_path)
        .map_err(|source| ConfigError::Read { path: runtime_path.to_owned(), source })?;
    if worker_runtime_identity != runtime_identity {
        return Err(ConfigError::InvalidRuntimeWorkerConfig(
            "Admin 与 worker_config_file 指向的 Worker 必须共用同一 runtime_config_file",
        ));
    }
    Ok(file.worker.mail_delivery_budget())
}

#[cfg(test)]
mod tests {
    use super::reject_unsupported_role_sections;
    use crate::{ConfigError, Role};

    #[test]
    fn role_files_reject_sections_the_role_never_reads() {
        let cases = [
            (Role::Public, "worker"),
            (Role::Admin, "realtime"),
            (Role::Worker, "listener"),
            (Role::Worker, "http_security"),
            (Role::Worker, "realtime"),
            (Role::Worker, "session"),
        ];
        for (role, section) in cases {
            let contents = format!("common_file = \"common.toml\"\n[{section}]\n");
            let error = reject_unsupported_role_sections(role, &contents)
                .expect_err("该角色从不读取此区块，必须失败关闭");
            assert!(
                matches!(error, ConfigError::UnsupportedRoleSection { role: rejected, section: rejected_section }
                    if rejected == role.name() && rejected_section == section),
                "{role:?} [{section}] 的错误必须指明角色与区块"
            );
        }
    }

    #[test]
    fn sections_owned_by_the_role_stay_accepted() {
        let cases = [
            (Role::Public, "listener"),
            (Role::Public, "http_security"),
            (Role::Public, "session"),
            (Role::Public, "realtime"),
            (Role::Admin, "listener"),
            (Role::Admin, "http_security"),
            (Role::Admin, "session"),
            (Role::Admin, "worker"),
            (Role::Worker, "worker"),
        ];
        for (role, section) in cases {
            let contents = format!("common_file = \"common.toml\"\n[{section}]\n");
            assert!(
                reject_unsupported_role_sections(role, &contents).is_ok(),
                "{role:?} 应接受 [{section}]"
            );
        }
    }

    #[test]
    fn admin_role_file_rejects_the_public_session_limit() {
        let contents = "common_file = \"common.toml\"\n[session]\nidle_ttl_seconds = 900\nmax_public_sessions = 10\n";
        let error = reject_unsupported_role_sections(Role::Admin, contents)
            .expect_err("Admin 不消费公开会话上限");
        assert!(matches!(error, ConfigError::AdminSessionLimitNotApplied));

        // Public 与 runtime 仍拥有该字段，不能被同一条规则误伤。
        assert!(reject_unsupported_role_sections(Role::Public, contents).is_ok());
    }

    #[test]
    fn malformed_toml_defers_to_the_serde_error() {
        assert!(reject_unsupported_role_sections(Role::Worker, "[listener").is_ok());
    }
}
