//! 命令、参数与默认值的唯一定义；不读取配置、不连接任何外部资源。

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

mod database;
mod operations;

#[cfg(feature = "postgres")]
pub(crate) use database::RebuildModeArg;
pub(crate) use database::{BackupCommand, DbCommand};
pub(crate) use operations::{
    CredentialCommand, ForumCommand, FundsCommand, MailCommand, PmCommand, RulesCommand,
};
#[cfg(feature = "postgres")]
pub(crate) use operations::{
    CredentialKindArg, CredentialStatusArg, CredentialWorldArg, FundsWriteArgs, KeySourceArg,
};

#[derive(Debug, Parser)]
#[command(
    name = "auction-cli",
    version,
    about = "Auction 运维命令行",
    long_about = "Auction 运维命令行：配置与部署预检、systemd 启停编排、数据库迁移/建库/备份恢复，\
以及凭据、规则、论坛、装备 PM、邮件证据和 Bot 资金等显式运维入口。\n\
结构化结果写入 stdout（JSON），进度与诊断写入 stderr。"
)]
pub(crate) struct Cli {
    /// 三角色配置目录，须包含 public.toml、admin.toml 与 worker.toml；相对路径按当前目录解析。
    #[arg(long, global = true, value_name = "DIR", default_value = "/etc/auction")]
    pub(crate) config_dir: PathBuf,
    #[command(subcommand)]
    pub(crate) command: Command,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// 单角色配置的安全投影与三角色部署一致性检查；不连接数据库。
    #[command(subcommand)]
    Config(ConfigCommand),
    /// 分体 systemd 部署的状态、顺序启动、停止与重启。
    Service(ServiceArgs),
    /// 数据库状态、迁移、建库、恢复预检、保留盘点与破坏性重建。
    #[command(subcommand)]
    Db(DbCommand),
    /// PostgreSQL custom-format 备份、离线校验与单事务恢复。
    #[command(subcommand)]
    Backup(BackupCommand),
    /// 外部 Cookie 凭据的安全列表与 stdin 导入。
    #[command(subcommand)]
    Credential(CredentialCommand),
    /// 公开规则目录的首次导入。
    #[command(subcommand)]
    Rules(RulesCommand),
    /// 论坛未知写入的恢复清单与人工重试授权。
    #[command(subcommand)]
    Forum(ForumCommand),
    /// 装备超价提醒论坛私信的人工对账。
    #[command(subcommand)]
    Pm(PmCommand),
    /// MoogleMail 加密诊断证据导出与 Sent 只读核查。
    #[command(subcommand)]
    Mail(MailCommand),
    /// Bot 资金账户的只读预览、只记账开户与现金对账。
    #[command(subcommand)]
    Funds(FundsCommand),
}

#[derive(Debug, Subcommand)]
pub(crate) enum ConfigCommand {
    /// 加载单个角色配置并输出不含任何秘密的有效配置投影。
    Show {
        /// 要加载的角色文件：<config-dir>/<role>.toml。
        #[arg(value_enum)]
        role: RoleArg,
    },
    /// 联合加载三角色配置，核对共享目标、密钥拓扑、监听冲突与文件权限。
    Check {
        /// 把权限警告升级为失败；生产部署门禁必须使用。
        #[arg(long)]
        strict: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum RoleArg {
    Public,
    Admin,
    Worker,
}

impl RoleArg {
    pub(crate) const fn role(self) -> auction_config::Role {
        match self {
            Self::Public => auction_config::Role::Public,
            Self::Admin => auction_config::Role::Admin,
            Self::Worker => auction_config::Role::Worker,
        }
    }
}

#[derive(Debug, Args)]
pub(crate) struct ServiceArgs {
    #[command(flatten)]
    pub(crate) units: UnitArgs,
    #[command(subcommand)]
    pub(crate) action: ServiceAction,
}

#[derive(Debug, Subcommand)]
pub(crate) enum ServiceAction {
    /// 显示三个 unit 的 systemd 状态与应用 readiness；任一角色未就绪时以非零退出。
    Status,
    /// 严格预检、执行迁移，再按 Worker → Admin → Public 逐个启动并等待 readiness。
    Start {
        /// 每个角色等待 readiness 的秒数。
        #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(u64).range(1..=300))]
        startup_timeout_seconds: u64,
    },
    /// 按 Public → Admin → Worker 顺序停止三个 unit。
    Stop,
    /// 先预检新配置，再反序停止并执行与 start 相同的迁移和顺序启动。
    Restart {
        /// 每个角色等待 readiness 的秒数。
        #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(u64).range(1..=300))]
        startup_timeout_seconds: u64,
    },
}

/// 分体部署的宿主机边界；unit 名称只作为参数传入，不进入业务配置。
#[derive(Debug, Clone, Args)]
pub(crate) struct UnitArgs {
    /// systemctl 可执行文件的绝对路径。
    #[arg(long, value_name = "PATH", default_value = "/usr/bin/systemctl")]
    pub(crate) systemctl: PathBuf,
    /// 部署控制锁所在的私有状态目录；不存在时由 service 命令以 0700 创建。
    #[arg(long, value_name = "DIR", default_value = "/var/lib/auction/deployment")]
    pub(crate) state_dir: PathBuf,
    /// Worker systemd unit。
    #[arg(long, value_name = "UNIT", default_value = "auction-worker.service")]
    pub(crate) worker_unit: String,
    /// Admin API systemd unit。
    #[arg(long, value_name = "UNIT", default_value = "auction-admin-api.service")]
    pub(crate) admin_unit: String,
    /// Public API systemd unit。
    #[arg(long, value_name = "UNIT", default_value = "auction-public-api.service")]
    pub(crate) public_unit: String,
}

/// 写数据库的维护命令共用的停机门禁参数。
#[derive(Debug, Clone, Args)]
pub(crate) struct MaintenanceArgs {
    #[command(flatten)]
    pub(crate) units: UnitArgs,
    /// 明确声明本机没有用 systemd 管理三角色（容器或开发环境），跳过 unit 停止检查；
    /// 合并部署的 owner.lock 检查不受影响。
    #[arg(long)]
    pub(crate) no_systemd: bool,
}
