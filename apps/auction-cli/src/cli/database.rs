//! 数据库与备份恢复命令的参数定义。

use std::path::PathBuf;

use clap::{Subcommand, ValueEnum};

use super::MaintenanceArgs;

#[derive(Debug, Subcommand)]
pub(crate) enum DbCommand {
    /// 只读输出连接目标的身份、迁移历史与当前二进制期望版本。
    Status,
    /// 在确认三角色均已停止后执行内嵌的追加式迁移；合并部署须先停止 auction-server。
    Migrate {
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
    /// 在有界只读快照检查内部不变量，仅追加巡检审计事实，不改变资金或资产。
    InspectConsistency {
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
        #[command(flatten)]
        maintenance: MaintenanceArgs,
    },
}
