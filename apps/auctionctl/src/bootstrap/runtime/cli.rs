//! auctionctl 命令行参数和稳定命令枚举。

use super::*;

#[derive(Debug, Parser)]
#[command(name = "auctionctl", about = "Auction Rust 运维命令")]
pub(super) struct Cli {
    #[command(subcommand)]
    pub(super) command: Command,
}

#[derive(Debug, Subcommand)]
pub(super) enum Command {
    CheckConfig {
        #[arg(value_enum, default_value = "public")]
        role: RoleArg,
    },
    /// 离线解密一个 MoogleMail 异常证据文件并导出 JSON；不会连接数据库或远端服务。
    #[command(name = "mail-evidence")]
    MailEvidence {
        #[command(subcommand)]
        action: MailEvidenceAction,
    },
    /// 在确认 Public/Admin/Worker 均未运行后执行数据库 forward-only 迁移。
    Migrate {
        #[command(flatten)]
        controller: SystemdControllerArgs,
    },
    /// 在全部角色明确停止后，安全创建缺失的 PostgreSQL 目标库；不会运行迁移或修改已有库。
    #[command(name = "postgres-bootstrap")]
    PostgresBootstrap {
        #[command(flatten)]
        controller: SystemdControllerArgs,
        /// 人工确认的目标数据库名；必须与 Public database_url 的 path 完全一致。
        #[arg(long)]
        confirm_database: String,
        /// 可选的 owner-only maintenance TOML；唯一字段 database_url 必须指向同 endpoint/TLS profile 的 postgres 库。
        #[arg(long)]
        maintenance_secrets_file: Option<PathBuf>,
    },
    /// 只读检查三进程配置、数据库档位、监听地址和文件安全边界。
    #[command(name = "deployment-check")]
    DeploymentCheck {
        /// 包含 common.toml、public.toml、admin.toml 和 worker.toml 的部署配置目录。
        #[arg(long, default_value = "ops/config/examples")]
        config_dir: PathBuf,
        /// 严格要求部署目录、公共/角色/runtime TOML 和敏感 TOML 的权限边界；默认仅报告警告，便于检查示例目录。
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
    /// 将 Auction-BF 规则目录首次导入 PostgreSQL 权威规则表；不会覆盖不同的已发布正文。
    #[command(name = "import-rules-directory")]
    ImportRulesDirectory {
        /// 包含 en、zh-CN、zh-TW、ja、ko 五个 Markdown 文件的目录。
        #[arg(long)]
        directory: PathBuf,
        /// 本次导入批次的非零 UUID；各语言命令 ID 由它稳定派生。
        #[arg(long)]
        command_id: Uuid,
        /// 写入规则发布审计的非空运维操作者标识。
        #[arg(long)]
        actor: String,
    },
    /// 只读列出远端论坛写入未知、只读对账中和等待人工授权的任务。
    #[command(name = "forum-write-recovery-list")]
    ForumWriteRecoveryList {
        /// 最多返回的恢复项数，范围 1-100。
        #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u16).range(1..=100))]
        limit: u16,
    },
    /// 在精确标题只读对账未命中后，显式授权 Worker 再次尝试创建论坛主题。
    #[command(name = "authorize-forum-write-retry")]
    AuthorizeForumWriteRetry {
        /// 等待人工授权的拍卖 UUID。
        #[arg(long)]
        auction_id: Uuid,
        /// 本次幂等运维命令 UUID；重复提交只能重放同一请求。
        #[arg(long)]
        command_id: Uuid,
        /// 写入追加式审计记录的非空运维操作者标识。
        #[arg(long)]
        actor: String,
        /// 二次确认，必须逐字等于 `AUTHORIZE FORUM WRITE <auction UUID>`。
        #[arg(long)]
        confirm_write: String,
    },
    /// 在同一 PostgreSQL Serializable 只读快照中核验旧版已完成结算账本与当前权威事实。
    #[command(name = "verify-legacy-settlement")]
    VerifyLegacySettlement {
        /// 已迁移且状态为 settled 的拍卖 UUID；该命令不会补写或重新执行结算。
        #[arg(long)]
        auction_id: Uuid,
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

#[derive(Debug, Subcommand)]
pub(super) enum MailEvidenceAction {
    /// 按证据 UUID 导出 MoogleMail 异常原始数据；输出文件已存在时拒绝覆盖。
    Export {
        /// 诊断证据 UUID；只按 UUID 定位归档文件。
        evidence_id: Uuid,
        /// 新建的 JSON 输出文件路径。
        #[arg(long)]
        output: PathBuf,
        /// 可选历史父 key 来源；省略时优先 credential，缺失时使用 equipment。
        #[arg(long, value_enum)]
        key_source: Option<MailEvidenceKeySourceArg>,
    },
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub(super) enum MailEvidenceKeySourceArg {
    Credential,
    Equipment,
}

#[derive(Debug, Args)]
pub(super) struct SystemdControllerArgs {
    /// systemctl 可执行文件；必须是绝对路径并解析到可执行普通文件。
    #[arg(long, default_value = "/usr/bin/systemctl")]
    pub(super) systemctl: PathBuf,
    /// 私有控制状态目录；必须预先创建为规范化绝对路径且不得允许 group/other 访问。
    #[arg(long, default_value = "/var/lib/auction/deployment")]
    pub(super) state_dir: PathBuf,
    /// Worker systemd unit 名称。
    #[arg(long, default_value = "auction-worker.service")]
    pub(super) worker_unit: String,
    /// Admin API systemd unit 名称。
    #[arg(long, default_value = "auction-admin-api.service")]
    pub(super) admin_unit: String,
    /// Public API systemd unit 名称。
    #[arg(long, default_value = "auction-public-api.service")]
    pub(super) public_unit: String,
}

#[derive(Debug, Subcommand)]
pub(super) enum DeploymentAction {
    /// 严格预检、迁移数据库并依次启动三个角色；失败只回滚本次启动的 unit。
    Start {
        /// 包含 common.toml、public.toml、admin.toml 和 worker.toml 的绝对部署配置目录。
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
        /// 包含 common.toml、public.toml、admin.toml 和 worker.toml 的绝对部署配置目录。
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
pub(super) enum RoleArg {
    Public,
    Admin,
    Worker,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub(super) enum PostgresRebuildModeArg {
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
