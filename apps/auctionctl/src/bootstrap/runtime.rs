use anyhow::Context;
#[cfg(feature = "postgres")]
use auction_application::{
    AdminCredentialImportCommand, AdminCredentialKind, AdminCredentialSecret,
    AdminCredentialService, AdminCredentialWorld,
};
use auction_config::{AppConfig, HttpListenMode, Role};
#[cfg(feature = "postgres")]
use auction_integrations::{CredentialSealer, DataKey};
use auction_storage::DatabaseBackend;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use auction_storage::DatabaseStore;
#[cfg(feature = "postgres")]
use chrono::{DateTime, Utc};
use clap::{Args, Parser, Subcommand, ValueEnum};
#[cfg(feature = "postgres")]
use serde::{Deserialize, Serialize};
#[cfg(feature = "postgres")]
use sha2::{Digest, Sha256};
#[cfg(feature = "postgres")]
use std::collections::HashSet;
#[cfg(feature = "postgres")]
use std::io::{Read, Write};
#[cfg(all(feature = "postgres", unix))]
use std::os::unix::fs::OpenOptionsExt;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(feature = "postgres")]
use std::process::{Command as ProcessCommand, Stdio};
#[cfg(feature = "postgres")]
use std::sync::Arc;
use std::{
    fs,
    net::SocketAddr,
    path::{Path, PathBuf},
};
#[cfg(feature = "postgres")]
use url::Url;
#[cfg(feature = "postgres")]
use uuid::Uuid;

mod deployment_control;
#[cfg(feature = "postgres")]
mod postgres_legacy_v28;
#[cfg(feature = "postgres")]
mod postgres_rebuild;
#[cfg(feature = "postgres")]
mod postgres_restore;

#[derive(Debug, Parser)]
#[command(name = "auctionctl", about = "Auction Rust 运维命令")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    CheckConfig {
        #[arg(value_enum, default_value = "public")]
        role: RoleArg,
    },
    Migrate,
    /// 只读检查三进程配置、数据库档位、监听地址和文件安全边界。
    #[command(name = "deployment-check")]
    DeploymentCheck {
        /// 包含 public.toml、admin.toml 和 worker.toml 的部署配置目录。
        #[arg(long, default_value = "ops/config/examples")]
        config_dir: PathBuf,
        /// 严格要求敏感 TOML 不包含 group/other 权限；默认仅报告警告，便于检查示例目录。
        #[arg(long)]
        strict_permissions: bool,
    },
    /// 按 Worker → Admin → Public 顺序控制已安装的 systemd unit。
    Deployment {
        #[command(flatten)]
        controller: SystemdControllerArgs,
        #[command(subcommand)]
        action: DeploymentAction,
    },
    #[command(name = "bootstrap-mail-credentials")]
    BootstrapMailCredentials {
        /// 已在 Worker 配置中启用的 MoogleMail world，只允许 main 或 isekai。
        #[arg(long)]
        world: String,
        /// 凭据在 Admin 和 Worker 中显示的稳定标签。
        #[arg(long)]
        label: String,
    },
    /// 只读收集 PostgreSQL schema、迁移和关系大小/行数估计；不会归档、删除或修改数据库。
    #[command(name = "retention-inventory")]
    RetentionInventory {
        /// 不可覆盖的绝对证据文件路径，父目录必须已经存在且不得允许 group/other 写入。
        #[arg(long)]
        output: PathBuf,
        /// 人工确认的目标 PostgreSQL 数据库名；必须与连接实际返回的 current_database 一致。
        #[arg(long)]
        confirm_database: String,
    },
    /// 只读检查 PostgreSQL 恢复目标；不会执行 pg_restore、创建或删除数据库。
    #[command(name = "recovery-preflight")]
    RecoveryPreflight {
        /// 不可覆盖的绝对证据文件路径，父目录必须已经存在且不得允许 group/other 写入。
        #[arg(long)]
        output: PathBuf,
        /// 人工确认的目标 PostgreSQL 数据库名；必须与连接实际返回的 current_database 一致。
        #[arg(long)]
        confirm_database: String,
    },
    /// 创建并校验 PostgreSQL custom-format 备份；不会修改数据库。
    #[command(name = "postgres-backup")]
    PostgresBackup {
        /// 新的 custom-format `.dump` 绝对路径；同名 archive/manifest 均拒绝覆盖。
        #[arg(long)]
        output: PathBuf,
        /// 人工确认的源数据库名；必须与连接实际返回的 current_database 一致。
        #[arg(long)]
        confirm_database: String,
    },
    /// 校验本地 PostgreSQL backup archive 与不可变 manifest；不会连接或修改数据库。
    #[command(name = "postgres-backup-verify")]
    PostgresBackupVerify {
        /// `.dump.manifest` 的绝对路径；archive 必须位于同一目录且使用 manifest 中的文件名。
        #[arg(long)]
        manifest: PathBuf,
        /// 人工确认的数据库名，用于核对 manifest 中的数据库身份摘要。
        #[arg(long)]
        confirm_database: String,
    },
    /// 为 canonical Auction-BF v28 创建专用 PostgreSQL custom-format 备份。
    #[command(name = "postgres-legacy-v28-backup")]
    PostgresLegacyV28Backup {
        /// 新的 custom-format `.dump` 绝对路径；同名 archive/manifest 均拒绝覆盖。
        #[arg(long)]
        output: PathBuf,
        /// 人工确认的旧版源数据库名；必须与连接实际返回的 current_database 一致。
        #[arg(long)]
        confirm_database: String,
    },
    /// 离线校验旧版 v28 archive、manifest、数据库身份摘要与 SHA-256。
    #[command(name = "postgres-legacy-v28-backup-verify")]
    PostgresLegacyV28BackupVerify {
        /// `.dump.manifest` 的绝对路径；archive 必须位于同一私有目录。
        #[arg(long)]
        manifest: PathBuf,
        /// 人工确认的旧版数据库名。
        #[arg(long)]
        confirm_database: String,
    },
    /// 严格预检并接管 canonical Auction-BF PostgreSQL v28；默认只读演练。
    #[command(name = "postgres-adopt-legacy-v28")]
    PostgresAdoptLegacyV28 {
        /// 人工确认的目标数据库名；必须与 Worker 连接目标完全一致。
        #[arg(long)]
        confirm_database: String,
        /// 专用旧版 v28 `.dump.manifest`；实际执行时必填。
        #[arg(long)]
        backup_manifest: Option<PathBuf>,
        /// 允许执行 schema/data bridge 和前向迁移；未提供时只读预检。
        #[arg(long)]
        execute: bool,
        /// 实际执行二次确认，必须逐字等于 `ADOPT LEGACY V28 <数据库名>`。
        #[arg(long)]
        confirm_adoption: Option<String>,
        /// 接管成功后发布的不可覆盖私有 JSON receipt；实际执行时必填。
        #[arg(long)]
        receipt: Option<PathBuf>,
    },
    /// 将已验证的 PostgreSQL custom-format archive 单事务恢复到已确认的空数据库。
    #[command(name = "postgres-restore")]
    PostgresRestore {
        /// `.dump.manifest` 的绝对路径；archive 必须位于同一目录且使用 manifest 中的文件名。
        #[arg(long)]
        manifest: PathBuf,
        /// 人工确认的目标数据库名；必须同时匹配连接目标与 manifest 数据库身份摘要。
        #[arg(long)]
        confirm_database: String,
        /// 实际恢复成功后发布的不可覆盖私有 JSON receipt；`--dry-run` 时不得提供。
        #[arg(long)]
        receipt: Option<PathBuf>,
        /// 只验证 archive、目标数据库和恢复前置条件，不执行 `pg_restore`。
        #[arg(long)]
        dry_run: bool,
    },
    /// 严格预检后删除并重建已确认的现有 Auction PostgreSQL 数据库；默认只读演练。
    #[command(name = "postgres-rebuild")]
    PostgresRebuild {
        /// 人工确认的目标数据库名；必须与 Worker 连接目标和 PostgreSQL catalog 同时匹配。
        #[arg(long)]
        confirm_database: String,
        /// 重建后保持空库等待 restore，或立即应用当前二进制的完整迁移。
        #[arg(long, value_enum, default_value = "fresh-schema")]
        mode: PostgresRebuildModeArg,
        /// 已验证 `.dump.manifest` 的绝对路径；与一次性环境豁免互斥。
        #[arg(long, conflicts_with = "allow_unrecoverable_ephemeral")]
        backup_manifest: Option<PathBuf>,
        /// 明确声明目标是可丢弃的一次性数据库，允许没有恢复 archive；不得用于持久数据。
        #[arg(long, conflicts_with = "backup_manifest")]
        allow_unrecoverable_ephemeral: bool,
        /// 一次性环境二次确认，必须逐字等于 `EPHEMERAL <数据库名>`。
        #[arg(long)]
        confirm_ephemeral: Option<String>,
        /// 独立 owner-only 敏感 TOML；只含指向同集群 `postgres` 维护库的 `database_url`。
        #[arg(long)]
        maintenance_secrets_file: Option<PathBuf>,
        /// 允许执行实际 DROP/CREATE；未提供时命令始终只读预检。
        #[arg(long)]
        execute: bool,
        /// 实际执行二次确认，必须逐字等于 `REBUILD <数据库名>`。
        #[arg(long)]
        confirm_rebuild: Option<String>,
        /// 实际发生破坏性 mutation 后发布的不可覆盖私有 JSON 回执。
        #[arg(long)]
        receipt: Option<PathBuf>,
    },
}

#[derive(Debug, Args)]
struct SystemdControllerArgs {
    /// systemctl 可执行文件；必须是绝对路径并解析到可执行普通文件。
    #[arg(long, default_value = "/usr/bin/systemctl")]
    systemctl: PathBuf,
    /// 私有控制状态目录；必须预先创建为规范化绝对路径且不得允许 group/other 访问。
    #[arg(long, default_value = "/var/lib/auction/deployment")]
    state_dir: PathBuf,
    /// Worker systemd unit 名称。
    #[arg(long, default_value = "auction-worker.service")]
    worker_unit: String,
    /// Admin API systemd unit 名称。
    #[arg(long, default_value = "auction-admin-api.service")]
    admin_unit: String,
    /// Public API systemd unit 名称。
    #[arg(long, default_value = "auction-public-api.service")]
    public_unit: String,
}

#[derive(Debug, Subcommand)]
enum DeploymentAction {
    /// 严格预检、迁移数据库并依次启动三个角色；失败只回滚本次启动的 unit。
    Start {
        /// 包含 public.toml、admin.toml 和 worker.toml 的绝对部署配置目录。
        #[arg(long, default_value = "/etc/auction")]
        config_dir: PathBuf,
        /// 每个角色等待应用 readiness 的秒数。
        #[arg(
            long,
            default_value_t = 30,
            value_parser = clap::value_parser!(u64).range(1..=300)
        )]
        startup_timeout_seconds: u64,
    },
    /// 按 Public → Admin → Worker 顺序停止三个 unit；停止超时由 unit 的 TimeoutStopSec 管理。
    Stop,
    /// 先反序停止，再执行与 start 相同的预检、迁移、启动和失败回滚。
    Restart {
        /// 包含 public.toml、admin.toml 和 worker.toml 的绝对部署配置目录。
        #[arg(long, default_value = "/etc/auction")]
        config_dir: PathBuf,
        /// 每个角色等待应用 readiness 的秒数。
        #[arg(
            long,
            default_value_t = 30,
            value_parser = clap::value_parser!(u64).range(1..=300)
        )]
        startup_timeout_seconds: u64,
    },
    /// 显示三个 systemd unit 的装载、活动、子状态、PID 和启动回执匹配状态。
    Status,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum RoleArg {
    Public,
    Admin,
    Worker,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum PostgresRebuildModeArg {
    /// 创建经过空库证明的同名数据库，随后必须显式运行 postgres-restore。
    EmptyForRestore,
    /// 创建空库后立即应用当前二进制内嵌的全部 PostgreSQL 迁移。
    FreshSchema,
}

impl From<RoleArg> for Role {
    fn from(value: RoleArg) -> Self {
        match value {
            RoleArg::Public => Self::Public,
            RoleArg::Admin => Self::Admin,
            RoleArg::Worker => Self::Worker,
        }
    }
}

const EXIT_USAGE_OR_CONFIG: u8 = 2;
const EXIT_DEPENDENCY: u8 = 3;
const EXIT_DEPLOYMENT: u8 = 4;
const EXIT_OPERATION: u8 = 5;

/// auctionctl 对外稳定的失败结果；错误正文沿用已脱敏的业务上下文，退出码由命令类别决定。
#[derive(Debug)]
pub struct AuctionCtlError {
    code: u8,
    source: anyhow::Error,
}

impl AuctionCtlError {
    fn new(code: u8, source: anyhow::Error) -> Self {
        Self { code, source }
    }

    /// 返回供 shell/systemd 使用的稳定退出码。
    pub const fn exit_code(&self) -> u8 {
        self.code
    }
}

impl std::fmt::Display for AuctionCtlError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{:#}", self.source)
    }
}

impl std::error::Error for AuctionCtlError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source.source()
    }
}

impl Command {
    fn failure_code(&self) -> u8 {
        match self {
            Self::CheckConfig { .. } | Self::DeploymentCheck { .. } => EXIT_USAGE_OR_CONFIG,
            Self::Deployment { .. } => EXIT_DEPLOYMENT,
            Self::Migrate
            | Self::RetentionInventory { .. }
            | Self::RecoveryPreflight { .. }
            | Self::PostgresBackup { .. }
            | Self::PostgresBackupVerify { .. }
            | Self::PostgresLegacyV28Backup { .. }
            | Self::PostgresLegacyV28BackupVerify { .. }
            | Self::PostgresAdoptLegacyV28 { .. }
            | Self::PostgresRestore { .. }
            | Self::PostgresRebuild { .. } => EXIT_DEPENDENCY,
            Self::BootstrapMailCredentials { .. } => EXIT_OPERATION,
        }
    }
}

pub async fn run() -> Result<(), AuctionCtlError> {
    let command = Cli::try_parse()
        .map_err(|error| {
            AuctionCtlError::new(EXIT_USAGE_OR_CONFIG, anyhow::anyhow!(error.to_string()))
        })?
        .command;
    let failure_code = command.failure_code();
    run_command(command).await.map_err(|error| AuctionCtlError::new(failure_code, error))
}

async fn run_command(command: Command) -> anyhow::Result<()> {
    match command {
        Command::CheckConfig { role } => check_config(role.into())?,
        Command::Migrate => {
            migrate().await?;
        }
        Command::DeploymentCheck { config_dir, strict_permissions } => {
            deployment_check(&config_dir, strict_permissions)?;
        }
        Command::Deployment { controller, action } => {
            let controller = deployment_control::ControllerOptions {
                systemctl: controller.systemctl,
                state_dir: controller.state_dir,
                worker_unit: controller.worker_unit,
                admin_unit: controller.admin_unit,
                public_unit: controller.public_unit,
            };
            match action {
                DeploymentAction::Start { config_dir, startup_timeout_seconds } => {
                    deployment_control::start(
                        controller,
                        config_dir,
                        std::time::Duration::from_secs(startup_timeout_seconds),
                    )
                    .await?;
                }
                DeploymentAction::Stop => deployment_control::stop(controller)?,
                DeploymentAction::Restart { config_dir, startup_timeout_seconds } => {
                    deployment_control::restart(
                        controller,
                        config_dir,
                        std::time::Duration::from_secs(startup_timeout_seconds),
                    )
                    .await?;
                }
                DeploymentAction::Status => deployment_control::status(controller)?,
            }
        }
        Command::BootstrapMailCredentials { world, label } => {
            bootstrap_mail_credentials(&world, &label).await?;
        }
        Command::RetentionInventory { output, confirm_database } => {
            retention_inventory(&output, &confirm_database).await?;
        }
        Command::RecoveryPreflight { output, confirm_database } => {
            recovery_preflight(&output, &confirm_database).await?;
        }
        Command::PostgresBackup { output, confirm_database } => {
            postgres_backup(&output, &confirm_database).await?;
        }
        Command::PostgresBackupVerify { manifest, confirm_database } => {
            postgres_backup_verify(&manifest, &confirm_database).await?;
        }
        Command::PostgresLegacyV28Backup { output, confirm_database } => {
            execute_postgres_legacy_v28_backup(&output, &confirm_database).await?;
        }
        Command::PostgresLegacyV28BackupVerify { manifest, confirm_database } => {
            verify_postgres_legacy_v28_backup(&manifest, &confirm_database)?;
        }
        Command::PostgresAdoptLegacyV28 {
            confirm_database,
            backup_manifest,
            execute,
            confirm_adoption,
            receipt,
        } => {
            execute_postgres_legacy_v28_adoption(
                &confirm_database,
                backup_manifest.as_deref(),
                execute,
                confirm_adoption.as_deref(),
                receipt.as_deref(),
            )
            .await?;
        }
        Command::PostgresRestore { manifest, confirm_database, receipt, dry_run } => {
            execute_postgres_restore(&manifest, receipt.as_deref(), &confirm_database, dry_run)
                .await?;
        }
        Command::PostgresRebuild {
            confirm_database,
            mode,
            backup_manifest,
            allow_unrecoverable_ephemeral,
            confirm_ephemeral,
            maintenance_secrets_file,
            execute,
            confirm_rebuild,
            receipt,
        } => {
            execute_postgres_rebuild(
                &confirm_database,
                mode,
                backup_manifest.as_deref(),
                allow_unrecoverable_ephemeral,
                confirm_ephemeral.as_deref(),
                maintenance_secrets_file.as_deref(),
                execute,
                confirm_rebuild.as_deref(),
                receipt.as_deref(),
            )
            .await?;
        }
    }

    Ok(())
}

fn check_config(role: Role) -> anyhow::Result<()> {
    let config = AppConfig::load(role).context("load configuration")?;
    let database_backend = config
        .database_url()
        .map(DatabaseBackend::from_url)
        .transpose()
        .context("validate database_url")?;
    let listener = config.listener.as_ref();
    let listener_mode = listener.map(|listener| listener.mode.name());
    let listener_socket_path = listener
        .filter(|listener| listener.mode == HttpListenMode::Unix)
        .map(|listener| listener.socket_path.as_str());
    let listener_runtime_directory = listener
        .filter(|listener| listener.mode == HttpListenMode::Unix)
        .map(|listener| listener.runtime_directory.as_str());
    let listener_group = listener
        .filter(|listener| listener.mode == HttpListenMode::Unix)
        .map(|listener| listener.group.as_str());
    let summary = serde_json::json!({
        "role": config.role.name(),
        "listener": {
            "mode": listener_mode,
            "address": config.listen_addr.map(|address| address.to_string()),
            "runtime_directory": listener_runtime_directory,
            "socket_path": listener_socket_path,
            "group": listener_group,
        },
        "database": {
            "configured": config.database_url().is_some(),
            "backend": database_backend.map(DatabaseBackend::name),
        },
        "cache": {
            "backend": config.cache.backend.name(),
            "public_rules_ttl_seconds": config.cache.public_rules_ttl_seconds,
        },
        "metrics": {
            "enabled": config.metrics.enabled,
        },
        "tracing": {
            "enabled": config.tracing.enabled,
            "endpoint_configured": !config.tracing.endpoint.is_empty(),
            "headers_configured": config.tracing_headers().is_some(),
            "queue_capacity": config.tracing.queue_capacity,
        },
        "http_security": {
            "allowed_origin_count": config.http_security.allowed_origins.len(),
            "trusted_proxy_cidr_count": config.http_security.trusted_proxy_cidrs.len(),
            "trusted_unix_proxy": config.http_security.trusted_unix_proxy,
            "cookie_secure": config.http_security.cookie_secure,
        },
        "session": {
            "idle_ttl_seconds": config.session.idle_ttl_seconds,
            "absolute_ttl_seconds": config.session.absolute_ttl_seconds,
            "max_public_sessions": config.session.max_public_sessions,
        },
        "worker": {
            "concurrency": config.worker_concurrency(),
            "poll_interval_seconds": config.worker_poll_interval_seconds(),
            "control_enabled": config.worker.control.enabled,
            "control_socket_path": config.worker.control.socket_path,
            "control_allowed_peer_uid_count": config.worker.control.allowed_peer_uids.len(),
            "control_allowed_peer_gid_count": config.worker.control.allowed_peer_gids.len(),
        },
        "realtime": {
            "consumer_key": config.realtime.consumer_key,
            "poll_interval_millis": config.realtime.poll_interval_millis,
            "client_buffer": config.realtime.client_buffer,
        },
        "admin_runtime_configured": config.admin_runtime.is_some(),
        "admin_credentials_configured": config.admin_credentials_configured(),
        "config_file": config.config_path.as_ref().map(|path| path.display().to_string()),
        "secrets_file_configured": config.secrets_path.is_some(),
        "runtime_config_file": config.runtime_config_path.as_ref().map(|path| path.display().to_string()),
    });
    println!("{}", serde_json::to_string_pretty(&summary)?);
    Ok(())
}

struct DeploymentConfig {
    role: Role,
    config: AppConfig,
    backend: DatabaseBackend,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum DeploymentListener {
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
fn deployment_check(config_dir: &Path, strict_permissions: bool) -> anyhow::Result<()> {
    if !config_dir.is_dir() {
        anyhow::bail!("deployment config directory is not a directory: {}", config_dir.display());
    }

    let mut configs = Vec::with_capacity(3);
    for (role, filename) in
        [(Role::Public, "public.toml"), (Role::Admin, "admin.toml"), (Role::Worker, "worker.toml")]
    {
        let path = config_dir.join(filename);
        require_regular_file(&path, "主配置")?;
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

fn public_deployment_listener(config: &AppConfig) -> anyhow::Result<DeploymentListener> {
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

fn admin_deployment_listener(config: &AppConfig) -> anyhow::Result<DeploymentListener> {
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
    let metadata =
        fs::metadata(path).with_context(|| format!("read {kind} metadata: {}", path.display()))?;
    if !metadata.is_file() {
        anyhow::bail!("{kind} is not a regular file: {}", path.display());
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
        let mode = fs::metadata(path)?.permissions().mode();
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

#[cfg(feature = "postgres")]
async fn bootstrap_mail_credentials(world: &str, label: &str) -> anyhow::Result<()> {
    let config = AppConfig::load(Role::Worker).context("load worker configuration")?;
    if !config.worker.mooglemail.enabled {
        anyhow::bail!("Worker 的 MoogleMail 功能未启用");
    }
    let credential_world = match world {
        "main" => AdminCredentialWorld::Main,
        "isekai" => AdminCredentialWorld::Isekai,
        _ => anyhow::bail!("bootstrap mail world 必须是 main 或 isekai"),
    };
    if !config
        .worker
        .mooglemail
        .configured_worlds()
        .iter()
        .any(|configured| configured.world_id == world)
    {
        anyhow::bail!("bootstrap mail world 必须已在 Worker 配置中启用");
    }

    let database_url =
        config.database_url().context("database_url is required for credential bootstrap")?;
    if !matches!(DatabaseBackend::from_url(database_url)?, DatabaseBackend::Postgres) {
        anyhow::bail!("bootstrap-mail-credentials 只支持 PostgreSQL 完整档");
    }
    let data_key_id = config
        .credential_data_key_id()
        .context("credential_data_key_id is required for credential bootstrap")?;
    let data_key_material = config
        .credential_data_key()
        .context("credential_data_key is required for credential bootstrap")?;
    let data_key = DataKey::from_base64(data_key_id, data_key_material)
        .map_err(|_| anyhow::anyhow!("credential data key is invalid"))?;
    let cookie = read_bootstrap_cookie().context("read bootstrap mail Cookie from stdin")?;
    let secret = AdminCredentialSecret::new(cookie)
        .map_err(|_| anyhow::anyhow!("bootstrap mail Cookie is invalid"))?;
    let store = Arc::new(
        DatabaseStore::connect(database_url)
            .await
            .context("connect database for credential bootstrap")?,
    );
    let sealer = Arc::new(CredentialSealer::new(Arc::new(data_key)));
    let service = AdminCredentialService::new(store, sealer);
    let credential = service
        .import(AdminCredentialImportCommand {
            command_id: Uuid::new_v4(),
            kind: AdminCredentialKind::MailCookie,
            world: credential_world,
            label: label.to_owned(),
            secret,
            actor_id: "auctionctl/bootstrap-mail-credentials".to_owned(),
            request_id: None,
            now: Utc::now(),
        })
        .await
        .context("register bootstrap mail credential")?;
    println!(
        "credential_id={} kind={} world={} label={} status={}",
        credential.id,
        credential.kind.as_str(),
        credential.world.as_str(),
        credential.label,
        credential.status.as_str(),
    );
    Ok(())
}

#[cfg(not(feature = "postgres"))]
async fn bootstrap_mail_credentials(_world: &str, _label: &str) -> anyhow::Result<()> {
    anyhow::bail!("bootstrap-mail-credentials 需要 PostgreSQL 完整档；请使用 --features postgres")
}

#[cfg(feature = "postgres")]
async fn retention_inventory(output: &Path, confirmed_database_name: &str) -> anyhow::Result<()> {
    validate_private_evidence_path(output)?;
    let config = AppConfig::load(Role::Worker).context("load worker configuration")?;
    let database_url =
        config.database_url().context("database_url is required for retention inventory")?;
    if !matches!(DatabaseBackend::from_url(database_url)?, DatabaseBackend::Postgres) {
        anyhow::bail!("retention-inventory 只支持 PostgreSQL 完整档");
    }
    let store = DatabaseStore::connect(database_url)
        .await
        .context("connect database for retention inventory")?;
    let inventory = match store {
        DatabaseStore::Postgres(postgres) => postgres
            .collect_retention_inventory(confirmed_database_name)
            .await
            .context("collect PostgreSQL retention inventory")?,
        #[cfg(feature = "sqlite")]
        DatabaseStore::Sqlite(_) => anyhow::bail!("retention-inventory 只支持 PostgreSQL 完整档"),
    };
    let evidence =
        serde_json::to_vec_pretty(&inventory).context("encode retention inventory evidence")?;
    write_private_evidence(output, &evidence)?;
    println!(
        "retention-inventory written path={} database={} relations={} policy_tables={} policy_sha256={} migration_current={}",
        output.display(),
        inventory.database_name,
        inventory.relation_count,
        inventory.policy_table_count,
        inventory.policy_sha256,
        inventory.migration.current,
    );
    Ok(())
}

#[cfg(feature = "postgres")]
async fn recovery_preflight(output: &Path, confirmed_database_name: &str) -> anyhow::Result<()> {
    validate_private_evidence_path(output)?;
    let config = AppConfig::load(Role::Worker).context("load worker configuration")?;
    let database_url =
        config.database_url().context("database_url is required for recovery preflight")?;
    if !matches!(DatabaseBackend::from_url(database_url)?, DatabaseBackend::Postgres) {
        anyhow::bail!("recovery-preflight 只支持 PostgreSQL 完整档");
    }
    let store = DatabaseStore::connect(database_url)
        .await
        .context("connect database for recovery preflight")?;
    let preflight = match store {
        DatabaseStore::Postgres(postgres) => postgres
            .collect_recovery_preflight(confirmed_database_name)
            .await
            .context("collect PostgreSQL recovery preflight")?,
        #[cfg(feature = "sqlite")]
        DatabaseStore::Sqlite(_) => anyhow::bail!("recovery-preflight 只支持 PostgreSQL 完整档"),
    };
    let evidence =
        serde_json::to_vec_pretty(&preflight).context("encode recovery preflight evidence")?;
    write_private_evidence(output, &evidence)?;
    println!(
        "recovery-preflight written path={} database={} passed={} active_sessions={} user_objects={}",
        output.display(),
        preflight.database_name,
        preflight.restore_preflight_passed,
        preflight.active_other_sessions,
        preflight.user_object_count,
    );
    Ok(())
}

#[cfg(not(feature = "postgres"))]
async fn recovery_preflight(_output: &Path, _confirmed_database_name: &str) -> anyhow::Result<()> {
    anyhow::bail!("recovery-preflight 需要 PostgreSQL 完整档；请使用 --features postgres")
}

#[cfg(feature = "postgres")]
const POSTGRES_BACKUP_FORMAT: &str = "auction-postgres-backup.v1";

#[cfg(feature = "postgres")]
const MAX_BACKUP_MANIFEST_BYTES: u64 = 64 * 1024;

#[cfg(feature = "postgres")]
#[derive(Debug, Deserialize, Serialize)]
struct PostgresBackupManifest {
    format: String,
    archive_file_name: String,
    database_name_sha256: String,
    archive_sha256: String,
    schema_version: i64,
    created_at: DateTime<Utc>,
}

#[cfg(feature = "postgres")]
async fn postgres_backup(output: &Path, confirmed_database_name: &str) -> anyhow::Result<()> {
    validate_private_evidence_path(output)?;
    require_dump_extension(output)?;
    let manifest_path = backup_manifest_path(output)?;
    validate_private_evidence_path(&manifest_path)?;
    require_absent_artifact(output, "backup archive")?;
    require_absent_artifact(&manifest_path, "backup manifest")?;

    let config = AppConfig::load(Role::Worker).context("load worker configuration")?;
    let database_url =
        config.database_url().context("database_url is required for PostgreSQL backup")?;
    if !matches!(DatabaseBackend::from_url(database_url)?, DatabaseBackend::Postgres) {
        anyhow::bail!("postgres-backup 只支持 PostgreSQL 完整档");
    }
    let store = DatabaseStore::connect(database_url)
        .await
        .context("connect database for PostgreSQL backup")?;
    let postgres = match store {
        DatabaseStore::Postgres(postgres) => postgres,
        #[cfg(feature = "sqlite")]
        DatabaseStore::Sqlite(_) => anyhow::bail!("postgres-backup 只支持 PostgreSQL 完整档"),
    };
    let before = postgres
        .collect_recovery_preflight(confirmed_database_name)
        .await
        .context("inspect backup source before pg_dump")?;
    let schema_version = require_backup_source(&before)?;
    let command_environment =
        PostgresCommandEnvironment::from_database_url(database_url, confirmed_database_name)?;
    let temporary_archive = create_private_temp_path(output, "auction-postgres-backup")?;
    let backup_result = async {
        run_pg_dump(&command_environment, &temporary_archive)?;
        verify_archive(&temporary_archive)?;
        let after = postgres
            .collect_recovery_preflight(confirmed_database_name)
            .await
            .context("inspect backup source after pg_dump")?;
        let after_schema_version = require_backup_source(&after)?;
        if after_schema_version != schema_version {
            anyhow::bail!(
                "database migration version changed during backup: before={schema_version} after={after_schema_version}"
            );
        }

        publish_private_artifact(&temporary_archive, output)?;
        let manifest = PostgresBackupManifest {
            format: POSTGRES_BACKUP_FORMAT.to_owned(),
            archive_file_name: output
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| anyhow::anyhow!("backup archive filename is not valid UTF-8"))?
                .to_owned(),
            database_name_sha256: sha256_text(confirmed_database_name),
            archive_sha256: sha256_file(output)?,
            schema_version,
            created_at: Utc::now(),
        };
        let manifest_bytes =
            serde_json::to_vec_pretty(&manifest).context("encode PostgreSQL backup manifest")?;
        write_private_evidence(&manifest_path, &manifest_bytes)?;
        Ok::<PostgresBackupManifest, anyhow::Error>(manifest)
    }
    .await;
    if backup_result.is_err() {
        remove_owned_temp(&temporary_archive);
    }
    let manifest = backup_result?;
    println!(
        "postgres-backup written archive={} manifest={} schema_version={} archive_sha256={}",
        output.display(),
        manifest_path.display(),
        manifest.schema_version,
        manifest.archive_sha256,
    );
    Ok(())
}

#[cfg(feature = "postgres")]
async fn postgres_backup_verify(
    manifest_path: &Path,
    confirmed_database_name: &str,
) -> anyhow::Result<()> {
    validate_private_evidence_path(manifest_path)?;
    let manifest = read_backup_manifest(manifest_path)?;
    validate_backup_manifest(&manifest, confirmed_database_name)?;
    let archive_path = manifest_archive_path(manifest_path, &manifest)?;
    validate_private_evidence_path(&archive_path)?;
    verify_private_regular_file(&archive_path, "backup archive")?;
    let actual_sha256 = sha256_file(&archive_path)?;
    if actual_sha256 != manifest.archive_sha256 {
        anyhow::bail!("backup archive SHA-256 does not match its manifest");
    }
    verify_archive(&archive_path)?;
    println!(
        "postgres-backup-verify passed archive={} manifest={} schema_version={} archive_sha256={}",
        archive_path.display(),
        manifest_path.display(),
        manifest.schema_version,
        manifest.archive_sha256,
    );
    Ok(())
}

#[cfg(feature = "postgres")]
async fn execute_postgres_restore(
    manifest_path: &Path,
    receipt_path: Option<&Path>,
    confirmed_database_name: &str,
    dry_run: bool,
) -> anyhow::Result<()> {
    postgres_restore::execute(manifest_path, receipt_path, confirmed_database_name, dry_run).await
}

#[cfg(feature = "postgres")]
async fn execute_postgres_legacy_v28_backup(
    output: &Path,
    confirmed_database_name: &str,
) -> anyhow::Result<()> {
    postgres_legacy_v28::backup(output, confirmed_database_name).await
}

#[cfg(feature = "postgres")]
fn verify_postgres_legacy_v28_backup(
    manifest_path: &Path,
    confirmed_database_name: &str,
) -> anyhow::Result<()> {
    postgres_legacy_v28::verify_backup_cli(manifest_path, confirmed_database_name)
}

#[cfg(feature = "postgres")]
async fn execute_postgres_legacy_v28_adoption(
    confirmed_database_name: &str,
    backup_manifest: Option<&Path>,
    execute: bool,
    confirm_adoption: Option<&str>,
    receipt: Option<&Path>,
) -> anyhow::Result<()> {
    postgres_legacy_v28::adopt(
        confirmed_database_name,
        backup_manifest,
        execute,
        confirm_adoption,
        receipt,
    )
    .await
}

#[cfg(feature = "postgres")]
#[allow(clippy::too_many_arguments)]
async fn execute_postgres_rebuild(
    confirmed_database_name: &str,
    mode: PostgresRebuildModeArg,
    backup_manifest: Option<&Path>,
    allow_unrecoverable_ephemeral: bool,
    confirm_ephemeral: Option<&str>,
    maintenance_secrets_file: Option<&Path>,
    execute: bool,
    confirm_rebuild: Option<&str>,
    receipt: Option<&Path>,
) -> anyhow::Result<()> {
    postgres_rebuild::execute(postgres_rebuild::Options {
        confirmed_database_name,
        mode: match mode {
            PostgresRebuildModeArg::EmptyForRestore => postgres_rebuild::Mode::EmptyForRestore,
            PostgresRebuildModeArg::FreshSchema => postgres_rebuild::Mode::FreshSchema,
        },
        backup_manifest,
        allow_unrecoverable_ephemeral,
        confirm_ephemeral,
        maintenance_secrets_file,
        execute,
        confirm_rebuild,
        receipt,
    })
    .await
}

#[cfg(not(feature = "postgres"))]
#[allow(clippy::too_many_arguments)]
async fn execute_postgres_rebuild(
    _confirmed_database_name: &str,
    _mode: PostgresRebuildModeArg,
    _backup_manifest: Option<&Path>,
    _allow_unrecoverable_ephemeral: bool,
    _confirm_ephemeral: Option<&str>,
    _maintenance_secrets_file: Option<&Path>,
    _execute: bool,
    _confirm_rebuild: Option<&str>,
    _receipt: Option<&Path>,
) -> anyhow::Result<()> {
    anyhow::bail!("postgres-rebuild 需要 PostgreSQL 完整档；请使用 --features postgres")
}

#[cfg(not(feature = "postgres"))]
async fn execute_postgres_restore(
    _manifest_path: &Path,
    _receipt_path: Option<&Path>,
    _confirmed_database_name: &str,
    _dry_run: bool,
) -> anyhow::Result<()> {
    anyhow::bail!("postgres-restore 需要 PostgreSQL 完整档；请使用 --features postgres")
}

#[cfg(not(feature = "postgres"))]
async fn execute_postgres_legacy_v28_backup(
    _output: &Path,
    _confirmed_database_name: &str,
) -> anyhow::Result<()> {
    anyhow::bail!("postgres-legacy-v28-backup 需要 PostgreSQL 完整档；请使用 --features postgres")
}

#[cfg(not(feature = "postgres"))]
fn verify_postgres_legacy_v28_backup(
    _manifest_path: &Path,
    _confirmed_database_name: &str,
) -> anyhow::Result<()> {
    anyhow::bail!(
        "postgres-legacy-v28-backup-verify 需要 PostgreSQL 完整档；请使用 --features postgres"
    )
}

#[cfg(not(feature = "postgres"))]
async fn execute_postgres_legacy_v28_adoption(
    _confirmed_database_name: &str,
    _backup_manifest: Option<&Path>,
    _execute: bool,
    _confirm_adoption: Option<&str>,
    _receipt: Option<&Path>,
) -> anyhow::Result<()> {
    anyhow::bail!("postgres-adopt-legacy-v28 需要 PostgreSQL 完整档；请使用 --features postgres")
}

#[cfg(not(feature = "postgres"))]
async fn postgres_backup(_output: &Path, _confirmed_database_name: &str) -> anyhow::Result<()> {
    anyhow::bail!("postgres-backup 需要 PostgreSQL 完整档；请使用 --features postgres")
}

#[cfg(not(feature = "postgres"))]
async fn postgres_backup_verify(
    _manifest_path: &Path,
    _confirmed_database_name: &str,
) -> anyhow::Result<()> {
    anyhow::bail!("postgres-backup-verify 需要 PostgreSQL 完整档；请使用 --features postgres")
}

#[cfg(feature = "postgres")]
fn require_dump_extension(path: &Path) -> anyhow::Result<()> {
    if path.extension().and_then(|extension| extension.to_str()) != Some("dump") {
        anyhow::bail!("PostgreSQL backup output must use the .dump extension");
    }
    Ok(())
}

#[cfg(feature = "postgres")]
fn backup_manifest_path(archive_path: &Path) -> anyhow::Result<PathBuf> {
    let filename = archive_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow::anyhow!("backup archive filename is not valid UTF-8"))?;
    Ok(archive_path.with_file_name(format!("{filename}.manifest")))
}

#[cfg(feature = "postgres")]
fn require_absent_artifact(path: &Path, label: &str) -> anyhow::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => anyhow::bail!("{label} already exists; refusing to overwrite"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("inspect {label}: {}", path.display())),
    }
}

#[cfg(feature = "postgres")]
fn create_private_temp_path(target: &Path, label: &str) -> anyhow::Result<PathBuf> {
    let parent =
        target.parent().ok_or_else(|| anyhow::anyhow!("private artifact parent is missing"))?;
    for _ in 0..8 {
        let candidate = parent.join(format!(".{label}-{}.tmp", Uuid::new_v4()));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        match options.open(&candidate) {
            Ok(_) => return Ok(candidate),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("create private temporary artifact: {}", candidate.display())
                });
            }
        }
    }
    anyhow::bail!("could not allocate a private temporary backup artifact")
}

#[cfg(feature = "postgres")]
fn publish_private_artifact(temporary: &Path, target: &Path) -> anyhow::Result<()> {
    require_absent_artifact(target, "backup archive")?;
    fs::hard_link(temporary, target)
        .with_context(|| format!("publish backup archive: {}", target.display()))?;
    fs::remove_file(temporary)
        .with_context(|| format!("remove backup temporary artifact: {}", temporary.display()))?;
    Ok(())
}

#[cfg(feature = "postgres")]
fn remove_owned_temp(path: &Path) {
    let _ = fs::remove_file(path);
}

#[cfg(feature = "postgres")]
fn verify_private_regular_file(path: &Path, label: &str) -> anyhow::Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("read {label} metadata: {}", path.display()))?;
    if !metadata.file_type().is_file() {
        anyhow::bail!("{label} is not a regular non-symlink file: {}", path.display());
    }
    #[cfg(unix)]
    if metadata.permissions().mode() & 0o077 != 0 {
        anyhow::bail!("{label} allows group/other access: {}", path.display());
    }
    if metadata.len() == 0 {
        anyhow::bail!("{label} is empty: {}", path.display());
    }
    Ok(())
}

#[cfg(feature = "postgres")]
fn read_backup_manifest(path: &Path) -> anyhow::Result<PostgresBackupManifest> {
    verify_private_regular_file(path, "backup manifest")?;
    let metadata = fs::metadata(path)?;
    if metadata.len() > MAX_BACKUP_MANIFEST_BYTES {
        anyhow::bail!("backup manifest exceeds the bounded evidence size");
    }
    let bytes =
        fs::read(path).with_context(|| format!("read backup manifest: {}", path.display()))?;
    serde_json::from_slice(&bytes).context("decode PostgreSQL backup manifest")
}

#[cfg(feature = "postgres")]
fn validate_backup_manifest(
    manifest: &PostgresBackupManifest,
    confirmed_database_name: &str,
) -> anyhow::Result<()> {
    if manifest.format != POSTGRES_BACKUP_FORMAT
        || manifest.schema_version <= 0
        || manifest.archive_file_name.is_empty()
        || manifest.archive_file_name != manifest.archive_file_name.trim()
        || manifest.archive_file_name.contains('/')
        || manifest.archive_file_name.contains('\\')
        || manifest.archive_file_name.chars().any(char::is_control)
        || manifest.archive_file_name.ends_with(".manifest")
        || !manifest.archive_file_name.ends_with(".dump")
        || !is_sha256_hex(&manifest.database_name_sha256)
        || !is_sha256_hex(&manifest.archive_sha256)
    {
        anyhow::bail!("backup manifest identity or path fields are invalid");
    }
    if manifest.database_name_sha256 != sha256_text(confirmed_database_name) {
        anyhow::bail!("confirmed database name does not match backup manifest");
    }
    Ok(())
}

#[cfg(feature = "postgres")]
fn manifest_archive_path(
    manifest_path: &Path,
    manifest: &PostgresBackupManifest,
) -> anyhow::Result<PathBuf> {
    let manifest_filename = manifest_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow::anyhow!("backup manifest filename is not valid UTF-8"))?;
    let expected_manifest_filename = format!("{}.manifest", manifest.archive_file_name);
    if manifest_filename != expected_manifest_filename {
        anyhow::bail!("backup manifest filename is not paired with its archive");
    }
    let parent = manifest_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("backup manifest parent is missing"))?;
    Ok(parent.join(&manifest.archive_file_name))
}

#[cfg(feature = "postgres")]
fn sha256_file(path: &Path) -> anyhow::Result<String> {
    let mut file =
        fs::File::open(path).with_context(|| format!("open artifact: {}", path.display()))?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let bytes_read = file.read(&mut buffer).context("read artifact for SHA-256")?;
        if bytes_read == 0 {
            break;
        }
        digest.update(&buffer[..bytes_read]);
    }
    Ok(hex_digest(digest.finalize().as_slice()))
}

#[cfg(feature = "postgres")]
fn sha256_text(value: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(value.as_bytes());
    hex_digest(digest.finalize().as_slice())
}

#[cfg(feature = "postgres")]
fn hex_digest(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(feature = "postgres")]
fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(feature = "postgres")]
fn require_backup_source(
    preflight: &auction_storage::PostgresRecoveryPreflight,
) -> anyhow::Result<i64> {
    let _identity = preflight
        .schema_identity
        .as_ref()
        .filter(|identity| identity.application == "auction" && identity.baseline_version > 0)
        .ok_or_else(|| {
            anyhow::anyhow!("backup source does not have a valid Auction schema identity")
        })?;
    let migration = preflight
        .migration
        .as_ref()
        .filter(|migration| {
            migration.all_success && migration.current && migration.latest_version > 0
        })
        .ok_or_else(|| {
            anyhow::anyhow!("backup source migration history is not current and clean")
        })?;
    Ok(migration.latest_version)
}

#[cfg(feature = "postgres")]
fn run_pg_dump(environment: &PostgresCommandEnvironment, output: &Path) -> anyhow::Result<()> {
    let mut command = ProcessCommand::new("pg_dump");
    command
        .args(["--no-password", "--format=custom", "--no-owner", "--no-privileges"])
        .arg(format!("--file={}", output.display()))
        .stdin(Stdio::null())
        .stdout(Stdio::null());
    environment.apply(&mut command);
    run_external_command(&mut command, "pg_dump")
}

#[cfg(feature = "postgres")]
fn verify_archive(path: &Path) -> anyhow::Result<()> {
    verify_private_regular_file(path, "backup archive")?;
    let mut command = ProcessCommand::new("pg_restore");
    command.args(["--list", "--no-password"]).arg(path).stdin(Stdio::null()).stdout(Stdio::null());
    run_external_command(&mut command, "pg_restore --list")
}

#[cfg(feature = "postgres")]
fn run_external_command(command: &mut ProcessCommand, label: &str) -> anyhow::Result<()> {
    let status = command.status().with_context(|| format!("start {label}"))?;
    if !status.success() {
        anyhow::bail!("{label} failed with status {status}");
    }
    Ok(())
}

#[cfg(feature = "postgres")]
struct PostgresCommandEnvironment {
    variables: Vec<(String, String)>,
    passfile_root: Option<PathBuf>,
    passfile: Option<PathBuf>,
}

#[cfg(feature = "postgres")]
impl PostgresCommandEnvironment {
    fn from_database_url(
        database_url: &str,
        confirmed_database_name: &str,
    ) -> anyhow::Result<Self> {
        let url = Url::parse(database_url).context("parse PostgreSQL database_url for libpq")?;
        if !matches!(url.scheme(), "postgres" | "postgresql")
            || url.fragment().is_some()
            || url.username().is_empty()
        {
            anyhow::bail!(
                "database_url is outside the secure PostgreSQL command connection profile"
            );
        }
        let raw_database = url.path().strip_prefix('/').unwrap_or(url.path());
        let database = decode_uri_component(raw_database)?;
        if database != confirmed_database_name || database.is_empty() || database.contains('/') {
            anyhow::bail!("database_url path does not match --confirm-database");
        }
        let username = decode_uri_component(url.username())?;
        let password = url.password().map(decode_uri_component).transpose()?;
        let host = url.host_str().map(decode_uri_component).transpose()?;
        if host.as_deref().is_some_and(|host| host.is_empty() || host.contains(',')) {
            anyhow::bail!("database_url host is empty or uses an unsupported multi-host form");
        }

        let mut variables = vec![
            ("PGDATABASE".to_owned(), confirmed_database_name.to_owned()),
            ("PGUSER".to_owned(), username.clone()),
            // 外部 PostgreSQL 工具不得继承机器级的无限连接等待或非 UTF-8 客户端编码。
            ("PGCONNECT_TIMEOUT".to_owned(), "5".to_owned()),
            ("PGCLIENTENCODING".to_owned(), "UTF8".to_owned()),
        ];
        if let Some(host) = &host {
            variables.push(("PGHOST".to_owned(), host.clone()));
        }
        if let Some(port) = url.port() {
            variables.push(("PGPORT".to_owned(), port.to_string()));
        }

        let mut seen_query_keys = HashSet::new();
        for (key, value) in url.query_pairs() {
            let key = key.into_owned();
            let value = value.into_owned();
            if !seen_query_keys.insert(key.clone()) {
                anyhow::bail!("database_url contains a duplicate PostgreSQL query parameter");
            }
            if (key == "ssl" && seen_query_keys.contains("sslmode"))
                || (key == "sslmode" && seen_query_keys.contains("ssl"))
            {
                anyhow::bail!("database_url cannot combine ssl and sslmode");
            }
            match key.as_str() {
                "application_name" => {
                    require_pg_value(&value, "application_name", 63)?;
                    variables.push(("PGAPPNAME".to_owned(), value));
                }
                "connect_timeout" => {
                    let timeout = value
                        .parse::<u16>()
                        .ok()
                        .filter(|timeout| (1..=300).contains(timeout))
                        .ok_or_else(|| anyhow::anyhow!("connect_timeout must be 1..=300"))?;
                    variables.retain(|(key, _)| key != "PGCONNECT_TIMEOUT");
                    variables.push(("PGCONNECT_TIMEOUT".to_owned(), timeout.to_string()));
                }
                "sslmode" => {
                    if !matches!(
                        value.as_str(),
                        "disable" | "allow" | "prefer" | "require" | "verify-ca" | "verify-full"
                    ) {
                        anyhow::bail!(
                            "database_url sslmode is not supported by the PostgreSQL command adapter"
                        );
                    }
                    variables.push(("PGSSLMODE".to_owned(), value));
                }
                "ssl" if value == "true" => {
                    variables.push(("PGSSLMODE".to_owned(), "require".to_owned()));
                }
                _ => anyhow::bail!(
                    "database_url query parameter is not supported by the backup adapter: {key}"
                ),
            }
        }

        let (passfile_root, passfile) = if let Some(password) = password {
            let root = std::env::temp_dir().join(format!("auctionctl-pgpass-{}", Uuid::new_v4()));
            let passfile_result: anyhow::Result<PathBuf> = (|| {
                fs::create_dir(&root).with_context(|| {
                    format!("create private PostgreSQL passfile directory: {}", root.display())
                })?;
                #[cfg(unix)]
                fs::set_permissions(&root, fs::Permissions::from_mode(0o700))
                    .context("restrict PostgreSQL passfile directory")?;
                let passfile = root.join("pgpass");
                let mut options = fs::OpenOptions::new();
                options.write(true).create_new(true);
                #[cfg(unix)]
                options.mode(0o600);
                let mut file = options.open(&passfile).with_context(|| {
                    format!("create private PostgreSQL passfile: {}", passfile.display())
                })?;
                let host_value = host.as_deref().unwrap_or("*");
                let port_value = url.port().map_or_else(|| "*".to_owned(), |port| port.to_string());
                let line = format!(
                    "{}:{}:{}:{}:{}\n",
                    pgpass_escape(host_value),
                    pgpass_escape(&port_value),
                    pgpass_escape(confirmed_database_name),
                    pgpass_escape(&username),
                    pgpass_escape(&password),
                );
                file.write_all(line.as_bytes()).context("write private PostgreSQL passfile")?;
                file.sync_all().context("sync private PostgreSQL passfile")?;
                Ok(passfile)
            })();
            match passfile_result {
                Ok(passfile) => (Some(root), Some(passfile)),
                Err(error) => {
                    let _ = fs::remove_file(root.join("pgpass"));
                    let _ = fs::remove_dir(&root);
                    return Err(error);
                }
            }
        } else {
            (None, None)
        };

        Ok(Self { variables, passfile_root, passfile })
    }

    fn apply(&self, command: &mut ProcessCommand) {
        const POSTGRES_ENVIRONMENT_KEYS: &[&str] = &[
            "PGAPPNAME",
            "PGCHANNELBINDING",
            "PGCLIENTENCODING",
            "PGCONNECT_TIMEOUT",
            "PGDATABASE",
            "PGGSSDELEGATION",
            "PGGSSENCMODE",
            "PGGSSLIB",
            "PGHOST",
            "PGHOSTADDR",
            "PGKRBSRVNAME",
            "PGLOADBALANCEHOSTS",
            "PGMAXPROTOCOLVERSION",
            "PGMINPROTOCOLVERSION",
            "PGOPTIONS",
            "PGPASSWORD",
            "PGPASSFILE",
            "PGPORT",
            "PGREQUIREAUTH",
            "PGREQUIREPEER",
            "PGREQUIRESSL",
            "PGSERVICE",
            "PGSERVICEFILE",
            "PGSSLCERT",
            "PGSSLCERTMODE",
            "PGSSLCOMPRESSION",
            "PGSSLCRL",
            "PGSSLCRLDIR",
            "PGSSLKEY",
            "PGSSLMAXPROTOCOLVERSION",
            "PGSSLMINPROTOCOLVERSION",
            "PGSSLMODE",
            "PGSSLNEGOTIATION",
            "PGSSLROOTCERT",
            "PGSSLSNI",
            "PGSYSCONFDIR",
            "PGTARGETSESSIONATTRS",
            "PGUSER",
        ];
        for key in POSTGRES_ENVIRONMENT_KEYS {
            command.env_remove(key);
        }
        for (key, value) in &self.variables {
            command.env(key, value);
        }
        if let Some(passfile) = &self.passfile {
            command.env("PGPASSFILE", passfile);
        }
    }
}

#[cfg(feature = "postgres")]
impl Drop for PostgresCommandEnvironment {
    fn drop(&mut self) {
        if let Some(passfile) = &self.passfile {
            let _ = fs::remove_file(passfile);
        }
        if let Some(root) = &self.passfile_root {
            let _ = fs::remove_dir(root);
        }
    }
}

#[cfg(feature = "postgres")]
fn require_pg_value(value: &str, name: &str, maximum_bytes: usize) -> anyhow::Result<()> {
    if value.is_empty()
        || value.trim() != value
        || value.len() > maximum_bytes
        || value.chars().any(char::is_control)
    {
        anyhow::bail!("PostgreSQL {name} is invalid");
    }
    Ok(())
}

#[cfg(feature = "postgres")]
fn decode_uri_component(value: &str) -> anyhow::Result<String> {
    let mut bytes = Vec::with_capacity(value.len());
    let raw = value.as_bytes();
    let mut index = 0;
    while index < raw.len() {
        if raw[index] == b'%' {
            if index + 2 >= raw.len() {
                anyhow::bail!("database_url contains an incomplete percent escape");
            }
            let high = hex_value(raw[index + 1])?;
            let low = hex_value(raw[index + 2])?;
            bytes.push((high << 4) | low);
            index += 3;
        } else {
            bytes.push(raw[index]);
            index += 1;
        }
    }
    let decoded = String::from_utf8(bytes).context("database_url contains invalid UTF-8")?;
    if decoded.chars().any(char::is_control) {
        anyhow::bail!("database_url contains a control character");
    }
    Ok(decoded)
}

#[cfg(feature = "postgres")]
fn hex_value(value: u8) -> anyhow::Result<u8> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        _ => anyhow::bail!("database_url contains an invalid percent escape"),
    }
}

#[cfg(feature = "postgres")]
fn pgpass_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace(':', "\\:")
}

#[cfg(feature = "postgres")]
fn validate_private_evidence_path(path: &Path) -> anyhow::Result<()> {
    if !path.is_absolute() || path == Path::new("/") {
        anyhow::bail!("private evidence output must be a non-root absolute path");
    }
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| anyhow::anyhow!("private evidence output parent is missing"))?;
    let canonical_parent = fs::canonicalize(parent)
        .with_context(|| format!("canonicalize output parent: {}", parent.display()))?;
    if canonical_parent != parent {
        anyhow::bail!("private evidence output parent must be a canonical non-symlink path");
    }
    if path.file_name().is_none() {
        anyhow::bail!("private evidence output filename is missing");
    }
    Ok(())
}

#[cfg(not(feature = "postgres"))]
async fn retention_inventory(_output: &Path, _confirmed_database_name: &str) -> anyhow::Result<()> {
    anyhow::bail!("retention-inventory 需要 PostgreSQL 完整档；请使用 --features postgres")
}

#[cfg(feature = "postgres")]
fn write_private_evidence(path: &Path, contents: &[u8]) -> anyhow::Result<()> {
    validate_private_evidence_path(path)?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| anyhow::anyhow!("retention inventory output parent is missing"))?;
    let metadata = fs::symlink_metadata(parent)
        .with_context(|| format!("read retention inventory parent: {}", parent.display()))?;
    if !metadata.file_type().is_dir() {
        anyhow::bail!("retention inventory output parent is not a directory: {}", parent.display());
    }
    #[cfg(unix)]
    if metadata.permissions().mode() & 0o022 != 0 {
        anyhow::bail!(
            "retention inventory output parent is group/other writable: {}",
            parent.display()
        );
    }

    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(path)
        .with_context(|| format!("create retention inventory evidence: {}", path.display()))?;
    file.write_all(contents).context("write retention inventory evidence")?;
    file.write_all(b"\n").context("terminate retention inventory evidence")?;
    file.sync_all().context("sync retention inventory evidence")?;
    Ok(())
}

#[cfg(feature = "postgres")]
fn read_bootstrap_cookie() -> anyhow::Result<String> {
    const MAX_COOKIE_BYTES: usize = 16 * 1024;
    let mut bytes = Vec::with_capacity(4096);
    std::io::stdin().take((MAX_COOKIE_BYTES + 1) as u64).read_to_end(&mut bytes)?;
    if bytes.len() > MAX_COOKIE_BYTES {
        anyhow::bail!("bootstrap mail Cookie 超过允许长度");
    }
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
        if bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
    }
    String::from_utf8(bytes).map_err(Into::into)
}

async fn migrate() -> anyhow::Result<()> {
    let config = AppConfig::load(Role::Public).context("load configuration for migration")?;
    migrate_config(&config).await
}

async fn migrate_config(config: &AppConfig) -> anyhow::Result<()> {
    let database_url = config.database_url().context("database_url is required for migration")?;

    #[cfg(any(feature = "postgres", feature = "sqlite"))]
    {
        let store =
            DatabaseStore::connect(database_url).await.context("connect database for migration")?;
        store.migrate().await.context("run database migrations")?;
        println!("database migrations applied for {}", store.backend());
        Ok(())
    }

    #[cfg(not(any(feature = "postgres", feature = "sqlite")))]
    {
        let _ = database_url;
        anyhow::bail!("auctionctl was built without a database feature; enable postgres or sqlite")
    }
}
