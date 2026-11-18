//! 应用配置文件定位、读取和输入模型装配。

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::{env, fs, io};

use auction_application::admin::{AdminRuntimeDocument, AdminRuntimeMailDeliveryBudget};
use auction_application::core::RequestHash;

use super::model::{AppConfig, BuildInput, CommonFileConfig, FileConfig, PublicSessionFileConfig};
use crate::reader::read_toml_file;
use crate::settings::backup::BackupConfig;
use crate::settings::errors::ConfigError;
use crate::settings::shutdown::ShutdownFileConfig;
use crate::settings::validation::{
    resolve_config_path, resolve_secrets_path, resolve_worker_paths,
};
use crate::settings::{
    AuctionConfig, CONFIG_FILE_ENV, DatabasePoolConfig, DatabaseProfilesConfig,
    DatabaseRolesConfig, MetricsConfig, ReadinessConfig, RealtimeConfig, Role, RoleSessionConfig,
    SessionConfig, TracingSettings, WorkerConfig,
};

/// 同源 Worker 角色文件的非敏感路径，供 Admin 装配 runtime 文件存储时加载部署基值。
#[derive(Clone, Copy)]
pub struct RuntimeWorkerConfigSource {
    path: [u8; libc::PATH_MAX as usize],
    len: usize,
}

impl RuntimeWorkerConfigSource {
    /// 有界读取 Worker 非敏感角色文件。
    pub(crate) fn read(&self) -> Result<RuntimeWorkerConfigFile<'_>, ConfigError> {
        let path = Path::new(OsStr::from_bytes(&self.path[..self.len]));
        RuntimeWorkerConfigFile::read(path)
    }
}

/// 角色文件的一次有界读取，指纹和配置解析使用同一份原文。
pub(crate) struct RuntimeWorkerConfigFile<'a> {
    path: &'a Path,
    contents: String,
}

/// 通过同源检查的 Worker 角色配置，用于装配候选。
pub(crate) struct RuntimeWorkerDeployment {
    path: PathBuf,
    file: FileConfig<SessionConfig>,
}

impl RuntimeWorkerDeployment {
    pub(crate) fn mail_delivery_budget(&self) -> AdminRuntimeMailDeliveryBudget {
        self.file.worker.mail_delivery_budget()
    }

    pub(crate) fn validate_runtime(self, runtime: AdminRuntimeDocument) -> Result<(), ConfigError> {
        AppConfig::from_file(
            Role::Worker,
            Some(self.path),
            self.file,
            RoleSessionConfig::for_role(Role::Worker),
            Some(runtime),
        )
        .map(|_| ())
    }
}

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
            None,
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
                Self::from_file(role, Some(path), file, session, None)
            }
            Role::Admin | Role::Worker => {
                let mut file: FileConfig<SessionConfig> = toml::from_str(&contents)
                    .map_err(|_| ConfigError::Parse { path: path.clone() })?;
                let session = file
                    .session
                    .take()
                    .map_or_else(|| RoleSessionConfig::for_role(role), RoleSessionConfig::Timed);
                Self::from_file(role, Some(path), file, session, None)
            }
        }
    }

    fn from_file<Session>(
        role: Role,
        config_path: Option<PathBuf>,
        file: FileConfig<Session>,
        session: RoleSessionConfig,
        runtime_override: Option<AdminRuntimeDocument>,
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
            .map(|path| load_runtime_worker_source(path, runtime_config_path.as_deref()))
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
            runtime_override,
        )
    }
}

/// 角色文件只接受该角色真正消费的区块。
///
/// 解析模型三角色共用，不属于该角色的区块（如 Worker 的 `[listener]`）会被接受却永不
/// 生效；读入后立即失败关闭，避免保留“看起来生效”的配置，也避免回退旧程序时被静默忽略。
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

fn load_runtime_worker_source(
    path: &Path,
    runtime_path: Option<&Path>,
) -> Result<RuntimeWorkerConfigSource, ConfigError> {
    let encoded = path.as_os_str().as_bytes();
    let mut buffer = [0; libc::PATH_MAX as usize];
    buffer
        .get_mut(..encoded.len())
        .ok_or(ConfigError::InvalidRuntimeWorkerConfig("worker_config_file 路径过长"))?
        .copy_from_slice(encoded);
    let runtime_path = runtime_path.ok_or(ConfigError::InvalidRuntimeWorkerConfig(
        "worker_config_file 要求 Admin 同时配置 runtime_config_file",
    ))?;
    RuntimeWorkerConfigFile::read(path)?.parse(runtime_path)?;
    Ok(RuntimeWorkerConfigSource { path: buffer, len: encoded.len() })
}

impl<'a> RuntimeWorkerConfigFile<'a> {
    fn read(path: &'a Path) -> Result<Self, ConfigError> {
        let contents = read_toml_file(path)
            .map_err(|source| ConfigError::Read { path: path.to_owned(), source })?;
        Ok(Self { path, contents })
    }

    pub(crate) fn fingerprint(&self) -> RequestHash {
        RequestHash::from_canonical_bytes(self.contents.as_bytes())
    }

    /// 从已读取的内容解析角色文件，并核对双方的 runtime 引用。
    pub(crate) fn parse(self, runtime_path: &Path) -> Result<RuntimeWorkerDeployment, ConfigError> {
        let Self { path, contents } = self;
        reject_unsupported_role_sections(Role::Worker, &contents)?;
        let file: FileConfig<SessionConfig> =
            toml::from_str(&contents).map_err(|_| ConfigError::Parse { path: path.to_owned() })?;
        if file.worker_config_file.is_some() {
            return Err(ConfigError::InvalidRuntimeWorkerConfig(
                "worker_config_file 必须指向 Worker 角色文件，不能包含另一个 worker_config_file",
            ));
        }
        let worker_runtime_path =
            file.runtime_config_file.clone().ok_or(ConfigError::InvalidRuntimeWorkerConfig(
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
        Ok(RuntimeWorkerDeployment { path: path.to_owned(), file })
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    use std::path::PathBuf;
    use std::task::{Context, Poll, Waker};

    use auction_application::admin::{
        AdminRuntimeAlertsConfig, AdminRuntimeConfigError, AdminRuntimeDocument,
        AdminRuntimeFileStore, AdminRuntimeOperationsConfig, AdminRuntimeWorldsConfig,
    };
    use auction_application::core::ApplicationError;
    use auction_time::Timestamp;
    use uuid::Uuid;

    use super::{load_runtime_worker_source, reject_unsupported_role_sections};
    use crate::{AppConfig, ConfigError, Role, TomlRuntimeFileStore};

    struct AlertFixture {
        directory: PathBuf,
        runtime_path: PathBuf,
        worker_path: PathBuf,
    }

    impl AlertFixture {
        fn new() -> Self {
            let directory =
                std::env::temp_dir().join(format!("auction-worker-alert-{}", Uuid::new_v4()));
            fs::DirBuilder::new().mode(0o700).create(&directory).unwrap();
            fs::write(directory.join("common.toml"), "").unwrap();
            let secrets = directory.join("secrets.toml");
            fs::write(
                &secrets,
                r#"database_url = "postgresql://auction_runtime@localhost/fixture"
mooglemail_cookie = "fixture=value"
equipment_data_key_id = "equipment-fixture"
equipment_data_key = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="
user_login_data_key_id = "login-fixture"
user_login_data_key = "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBE="
"#,
            )
            .unwrap();
            fs::set_permissions(secrets, fs::Permissions::from_mode(0o600)).unwrap();
            Self {
                runtime_path: directory.join("runtime.toml"),
                worker_path: directory.join("worker.toml"),
                directory,
            }
        }

        fn write_worker(&self, scan: bool, notify: bool, mail: bool, target: &str) {
            fs::write(&self.worker_path, format!(
                "common_file = \"common.toml\"\nsecrets_file = \"secrets.toml\"\n\
                 runtime_config_file = \"runtime.toml\"\n\
                 [worker.mooglemail]\nenabled = {mail}\n\
                 [worker.operations_alert]\nscan_enabled = {scan}\nnotify_enabled = {notify}\nworld_id = \"{target}\"\nrecipient = \"Operator\"\n"
            )).unwrap();
        }

        fn write_runtime(&self, document: &AdminRuntimeDocument) {
            fs::write(&self.runtime_path, toml::to_string(document).unwrap()).unwrap();
            fs::set_permissions(&self.runtime_path, fs::Permissions::from_mode(0o640)).unwrap();
        }

        fn store(&self) -> TomlRuntimeFileStore {
            let source =
                load_runtime_worker_source(&self.worker_path, Some(&self.runtime_path)).unwrap();
            let store = TomlRuntimeFileStore::new(&self.runtime_path, false, Some(source)).unwrap();
            read_current(&store);
            store
        }
    }

    impl Drop for AlertFixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.directory);
        }
    }

    fn read_current(store: &TomlRuntimeFileStore) -> String {
        let Poll::Ready(result) =
            store.current().as_mut().poll(&mut Context::from_waker(Waker::noop()))
        else {
            panic!("本地文件读取不挂起");
        };
        result.unwrap().fingerprint
    }

    fn managed_alerts(enabled: bool, recipient: &str) -> AdminRuntimeDocument {
        let mut operations = AdminRuntimeOperationsConfig::default();
        operations.alerts.managed = true;
        operations.alerts.enabled = enabled;
        operations.alerts.recipient = recipient.to_owned();
        AdminRuntimeDocument { operations: Some(operations), ..Default::default() }
    }

    fn rejection(store: &TomlRuntimeFileStore, candidate: &AdminRuntimeDocument) -> String {
        match store.validate_candidate(candidate, Timestamp::now()).expect_err("候选必须拒绝")
        {
            ApplicationError::AdminRuntime(AdminRuntimeConfigError::InvalidDocument(reason)) => {
                reason
            }
            error => panic!("期望 invalid_runtime_config，收到 {error}"),
        }
    }

    #[test]
    fn alert_candidate_requires_fresh_worker_role_file_only_for_alert_changes() {
        let fixture = AlertFixture::new();
        let current = managed_alerts(true, "Operator");
        fixture.write_runtime(&current);
        fixture.write_worker(true, false, true, "main");
        let store = fixture.store();
        let candidate = managed_alerts(true, "");
        assert!(store.validate_candidate(&candidate, Timestamp::now()).is_ok());

        fixture.write_worker(true, true, true, "main");
        assert_eq!(
            rejection(&store, &candidate),
            AdminRuntimeAlertsConfig::WORKER_CONFIG_CHANGED_HINT,
        );
        fixture.write_runtime(&candidate);
        assert!(matches!(
            AppConfig::load_from_path(Role::Worker, &fixture.worker_path),
            Err(ConfigError::InvalidOperationsAlertConfig(reason)) if reason.contains("recipient")
        ));
        fixture.write_runtime(&current);
        let mut unrelated = current;
        unrelated.allow_manual_early_open = true;
        assert!(store.validate_candidate(&unrelated, Timestamp::now()).is_ok());

        fs::write(&fixture.worker_path, "[worker.operations_alert\n").unwrap();
        assert!(matches!(
            AppConfig::load_from_path(Role::Worker, &fixture.worker_path),
            Err(ConfigError::Parse { .. })
        ));
        assert_eq!(
            rejection(&store, &candidate),
            AdminRuntimeAlertsConfig::WORKER_CONFIG_CHANGED_HINT,
        );
        assert!(store.validate_candidate(&unrelated, Timestamp::now()).is_ok());

        fs::remove_file(&fixture.worker_path).unwrap();
        assert_eq!(
            rejection(&store, &candidate),
            AdminRuntimeAlertsConfig::WORKER_CONFIG_UNREADABLE_HINT,
        );
        assert!(store.validate_candidate(&unrelated, Timestamp::now()).is_ok());
    }

    #[test]
    fn alert_candidate_and_worker_startup_reject_notify_without_mail_transport() {
        let fixture = AlertFixture::new();
        let current = managed_alerts(false, "");
        fixture.write_runtime(&current);
        fixture.write_worker(true, true, false, "main");
        let active = AppConfig::load_from_path(Role::Worker, &fixture.worker_path).unwrap();
        assert!(!active.worker.operations_alert.notify_enabled());
        let store = fixture.store();
        let candidate = AdminRuntimeDocument::default();
        let reason = rejection(&store, &candidate);
        assert!(reason.contains("mooglemail.enabled"), "{reason}");

        fixture.write_runtime(&candidate);
        let error = AppConfig::load_from_path(Role::Worker, &fixture.worker_path).unwrap_err();
        assert!(
            matches!(error, ConfigError::InvalidOperationsAlertConfig(message) if message == reason)
        );
    }

    #[test]
    fn runtime_alert_candidate_uses_deployed_worker_notification() {
        let fixture = AlertFixture::new();
        let current = managed_alerts(true, "Operator");
        fixture.write_runtime(&current);
        fixture.write_worker(false, true, true, "main");
        let store = fixture.store();
        let initial = AppConfig::load_from_path(Role::Worker, &fixture.worker_path).unwrap();
        assert!(initial.worker.operations_alert.notify_enabled());

        assert_eq!(
            rejection(&store, &AdminRuntimeDocument::default()),
            "启用主动通知时必须同时启用扫描"
        );
        assert_eq!(
            rejection(&store, &managed_alerts(true, "")),
            "启用告警通知时 recipient 必须是非空且不含控制字符的文本"
        );
        assert!(store.validate_candidate(&managed_alerts(false, ""), Timestamp::now()).is_ok());
    }

    #[test]
    fn unmanaging_mail_worlds_cannot_drop_the_deployed_notification_target() {
        let fixture = AlertFixture::new();
        let current = AdminRuntimeDocument {
            worlds: Some(AdminRuntimeWorldsConfig::default()),
            equipment_filter: Some(Default::default()),
            ..Default::default()
        };
        fixture.write_runtime(&current);
        fixture.write_worker(true, true, true, "isekai");
        let initial = AppConfig::load_from_path(Role::Worker, &fixture.worker_path).unwrap();
        assert!(initial.worker.operations_alert.notify_enabled());
        let store = fixture.store();
        let without_worker = TomlRuntimeFileStore::new(&fixture.runtime_path, false, None).unwrap();
        assert_eq!(read_current(&without_worker), current.fingerprint());

        let mut candidate = current.clone();
        candidate.worlds.as_mut().unwrap().isekai.enabled = false;
        for candidate in [candidate, AdminRuntimeDocument::default()] {
            assert_eq!(
                rejection(&store, &candidate),
                "启用告警通知时 world_id 必须位于已启用的 MoogleMail worlds"
            );
            assert_eq!(
                rejection(&without_worker, &candidate),
                AdminRuntimeAlertsConfig::WORKER_CONFIG_REQUIRED_HINT
            );
        }
        assert_eq!(
            rejection(&without_worker, &managed_alerts(false, "")),
            AdminRuntimeAlertsConfig::WORKER_CONFIG_REQUIRED_HINT
        );

        let mut unrelated = current;
        unrelated.allow_manual_early_open = true;
        unrelated.worlds.as_mut().unwrap().isekai.sender_allowlist.push("Operator".to_owned());
        assert!(without_worker.validate_candidate(&unrelated, Timestamp::now()).is_ok());
        assert!(store.validate_candidate(&unrelated, Timestamp::now()).is_ok());
    }

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
