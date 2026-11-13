//! 数据库与备份恢复命令的参数定义。

use std::path::PathBuf;

use auction_storage::RETENTION_PURGE_DEFAULT_BATCH_SIZE;
use clap::{Args, Subcommand, ValueEnum};

use super::MaintenanceArgs;

/// 迁移 owner 凭据：只允许 auction-cli 的建库、迁移、重建与恢复读取。
#[derive(Debug, Clone, Args)]
pub(crate) struct OwnerSecretsArg {
    /// owner-only TOML，只含指向应用数据库的 [database]；账号须为数据库 owner，不得与服务共用。
    #[arg(long, value_name = "FILE")]
    pub(crate) owner_secrets: PathBuf,
}

#[derive(Debug, Subcommand)]
pub(crate) enum DbCommand {
    /// 只读输出连接目标的身份、迁移历史与当前二进制期望版本。
    Status,
    /// 在确认三角色均已停止后以迁移 owner 执行内嵌的追加式迁移；合并部署须先停止 auction-server。
    Migrate {
        #[command(flatten)]
        owner: OwnerSecretsArg,
        #[command(flatten)]
        maintenance: MaintenanceArgs,
    },
    /// 以迁移 owner 重放运行账号最小权限（迁移 000081 的授权函数）；恢复后与授权变更时使用。
    Grants {
        #[command(flatten)]
        owner: OwnerSecretsArg,
        #[command(flatten)]
        maintenance: MaintenanceArgs,
    },
    /// 用独立维护凭据创建缺失的应用数据库；目标已存在时只校验 owner 与应用连接，不执行迁移。
    Create {
        /// 人工确认的数据库名，须与 Worker 配置中的库名一致。
        #[arg(long, value_name = "NAME")]
        database: String,
        /// owner-only 维护 TOML，只含指向同集群 postgres 库的 [database]。
        #[arg(long, value_name = "FILE")]
        maintenance_secrets: PathBuf,
        /// owner-only 迁移凭据，只含指向应用数据库的 [database]；新库 owner 必须是该账号。
        #[command(flatten)]
        owner: OwnerSecretsArg,
        #[command(flatten)]
        maintenance: MaintenanceArgs,
    },
    /// 只读检查恢复目标（空库、无其他连接、可写主节点），证据写入新的私有 JSON 文件。
    Preflight {
        /// 人工确认的数据库名，须与连接实际返回的 current_database 一致。
        #[arg(long, value_name = "NAME")]
        database: String,
        /// 不可覆盖的绝对证据路径；父目录须已存在且禁止 group/other 写入。
        #[arg(long, value_name = "FILE")]
        output: PathBuf,
    },
    /// 只读盘点 schema、迁移与关系规模，对照内嵌保留政策；结果不授权任何删除。
    Inventory {
        /// 人工确认的数据库名，须与连接实际返回的 current_database 一致。
        #[arg(long, value_name = "NAME")]
        database: String,
        /// 不可覆盖的绝对证据路径；父目录须已存在且禁止 group/other 写入。
        #[arg(long, value_name = "FILE")]
        output: PathBuf,
    },
    /// 只读演练保留清理：按政策 v2 统计运行态数据族的候选行与诊断文件数，写入带 run id 的回执；绝不执行删除。
    RetentionDryRun {
        /// 人工确认的数据库名，须与连接实际返回的 current_database 一致。
        #[arg(long, value_name = "NAME")]
        database: String,
        /// 不可覆盖的绝对回执路径；父目录须已存在且禁止 group/other 写入。
        #[arg(long, value_name = "FILE")]
        output: PathBuf,
    },
    /// 执行已批准的保留清理：绑定新鲜只读演练回执与逐数据族令牌，分批删除运行态候选。
    RetentionPurge {
        /// 人工确认的数据库名，须与只读演练回执及实际连接一致。
        #[arg(long, value_name = "NAME")]
        database: String,
        /// `db retention-dry-run` 写出的 owner-only 回执；须未过期且政策身份一致。
        #[arg(long, value_name = "FILE")]
        dry_run_receipt: PathBuf,
        /// 逐数据族批准：`<family>=<token>`；省略时只预览计划与令牌，不执行任何变更。
        #[arg(long = "approve", value_name = "FAMILY=TOKEN")]
        approvals: Vec<String>,
        /// 实际执行；必须同时提供确认串与回执路径。
        #[arg(long, requires_all = ["confirm", "receipt"])]
        execute: bool,
        /// 逐字确认：`PURGE <数据库名>`。
        #[arg(long, value_name = "TEXT", requires = "execute")]
        confirm: Option<String>,
        /// 实际执行后写入的不可覆盖私有 JSON 回执。
        #[arg(long, value_name = "FILE", requires = "execute")]
        receipt: Option<PathBuf>,
        /// 单批删除行数；每批一个独立事务。
        #[arg(long, value_name = "ROWS", default_value_t = RETENTION_PURGE_DEFAULT_BATCH_SIZE)]
        batch_size: u32,
    },
    /// 在有界只读快照检查内部不变量，仅追加巡检审计事实，不改变资金或资产。
    InspectConsistency {
        /// 人工确认的数据库名，须与 Worker 配置和实际连接一致。
        #[arg(long, value_name = "NAME")]
        database: String,
        /// 不可覆盖的私有 JSON 报告绝对路径；父目录须已存在。
        #[arg(long, value_name = "FILE")]
        output: PathBuf,
    },
    /// 只读输出最近追加的装备持有与偿付能力报告；不读取 Armory、不改变余额或装备。
    AssetReports {
        /// 人工确认的数据库名，须与 Worker 配置和实际连接一致。
        #[arg(long, value_name = "NAME")]
        database: String,
        /// 不可覆盖的私有 JSON 报告绝对路径；父目录须已存在。
        #[arg(long, value_name = "FILE")]
        output: PathBuf,
    },
    /// 仅从数据库投影追加一份偿付能力报告；不读取 Armory、不改账、不授权资金动作。
    ReportSolvency {
        /// 人工确认的数据库名，须与 Worker 配置和实际连接一致。
        #[arg(long, value_name = "NAME")]
        database: String,
        /// 不可覆盖的私有 JSON 报告绝对路径；父目录须已存在。
        #[arg(long, value_name = "FILE")]
        output: PathBuf,
    },
    /// 删除并以原 owner/编码/locale 重建已确认的数据库；默认只读演练。
    Rebuild {
        /// 人工确认的目标数据库名，须同时匹配 Worker 配置与集群 catalog。
        #[arg(long, value_name = "NAME")]
        database: String,
        /// owner-only 维护 TOML，只含指向同集群 postgres 库的 [database]。
        #[arg(long, value_name = "FILE")]
        maintenance_secrets: PathBuf,
        /// owner-only 迁移凭据；fresh-schema 重建后由该账号执行迁移。
        #[command(flatten)]
        owner: OwnerSecretsArg,
        /// 重建后保持空库等待恢复，或立即应用当前二进制的全部迁移。
        #[arg(long, value_enum, default_value = "fresh-schema")]
        mode: RebuildModeArg,
        /// 已通过校验的备份 manifest；与 --ephemeral 互斥，empty-for-restore 必须提供。
        #[arg(long, value_name = "FILE", conflicts_with = "ephemeral")]
        backup_manifest: Option<PathBuf>,
        /// 声明目标是可丢弃的一次性数据库，允许没有备份；不得用于持久数据。
        #[arg(long)]
        ephemeral: bool,
        /// 实际执行 DROP/CREATE；省略时只做持锁预检。
        #[arg(long, requires_all = ["confirm", "receipt"])]
        execute: bool,
        /// 逐字确认：`REBUILD <数据库名>`，--ephemeral 时为 `EPHEMERAL <数据库名>`。
        #[arg(long, value_name = "TEXT", requires = "execute")]
        confirm: Option<String>,
        /// 实际执行后写入的不可覆盖私有 JSON 回执（失败也会写入已知的破坏性状态）。
        #[arg(long, value_name = "FILE", requires = "execute")]
        receipt: Option<PathBuf>,
        #[command(flatten)]
        maintenance: MaintenanceArgs,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum RebuildModeArg {
    /// 只建立空库，随后必须执行 backup restore。
    EmptyForRestore,
    /// 建立空库后立即执行当前二进制的全部迁移。
    FreshSchema,
}

#[derive(Debug, Subcommand)]
pub(crate) enum BackupCommand {
    /// 定时逻辑备份：持久运行记录、磁盘门禁、create/verify、本地保留与终态回执。
    Scheduled {
        /// 备份源库名，必须与 Worker 配置及实际连接一致。
        #[arg(long, value_name = "NAME")]
        database: String,
        /// scheduled 专用私有目录；普通手动备份不会纳入清理。
        #[arg(long, value_name = "DIR")]
        output_dir: PathBuf,
        /// 私有运行状态、锁与不可变回执目录，必须与 output-dir 分离。
        #[arg(long, value_name = "DIR")]
        state_dir: PathBuf,
    },
    /// 用 pg_dump 生成 custom-format archive 与配对 manifest；不修改数据库。
    Create {
        /// 人工确认的源数据库名，须与连接实际返回的 current_database 一致。
        #[arg(long, value_name = "NAME")]
        database: String,
        /// 新的 `.dump` 绝对路径；同名 archive 与 `.dump.manifest` 均不得已存在。
        #[arg(long, value_name = "FILE")]
        output: PathBuf,
    },
    /// 离线校验 manifest、库名摘要、archive SHA-256 与 archive 目录；不连接数据库。
    Verify {
        /// `.dump.manifest` 的绝对路径；archive 须位于同一目录。
        #[arg(long, value_name = "FILE")]
        manifest: PathBuf,
        /// 人工确认的数据库名，用于核对 manifest 中的库名摘要。
        #[arg(long, value_name = "NAME")]
        database: String,
    },
    /// 离线解密并恢复校验备份范围（运行配置、邮件诊断证据、部署回执）；不覆盖已有文件。
    ScopeExtract {
        /// `.dump.manifest` 的绝对路径；同目录必须存在配对的范围 sidecar。
        #[arg(long, value_name = "FILE")]
        manifest: PathBuf,
        /// 人工确认的数据库名，用于核对 manifest 中的库名摘要。
        #[arg(long, value_name = "NAME")]
        database: String,
        /// 离线保管的 age 私钥文件；只作为解密参数，不复制内容。
        #[arg(long, value_name = "FILE")]
        identity: PathBuf,
        /// 必须为空或不存在的私有输出目录；逐个写入 `runtime_config/`、`mail_diagnostics/`、`deployment_receipts/`。
        #[arg(long, value_name = "DIR")]
        output_dir: PathBuf,
        /// 校验成功后写入的不可覆盖私有 JSON 回执。
        #[arg(long, value_name = "FILE")]
        receipt: Option<PathBuf>,
    },
    /// 把已校验的 archive 单事务恢复到同名空数据库。
    Restore {
        /// `.dump.manifest` 的绝对路径；archive 须位于同一目录。
        #[arg(long, value_name = "FILE")]
        manifest: PathBuf,
        /// 人工确认的目标数据库名，须同时匹配 Worker 配置与 manifest。
        #[arg(long, value_name = "NAME")]
        database: String,
        /// 只校验 archive 与目标空库条件，不执行 pg_restore。
        #[arg(long, conflicts_with = "receipt")]
        dry_run: bool,
        /// 恢复成功后写入的不可覆盖私有 JSON 回执；实际恢复时必填。
        #[arg(long, value_name = "FILE", required_unless_present = "dry_run")]
        receipt: Option<PathBuf>,
        /// owner-only 迁移凭据；pg_restore 需要 DDL 与对象所有权，不能使用服务运行账号。
        #[command(flatten)]
        owner: OwnerSecretsArg,
        #[command(flatten)]
        maintenance: MaintenanceArgs,
    },
}
