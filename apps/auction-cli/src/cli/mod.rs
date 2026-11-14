//! 命令、参数与默认值的唯一定义；不读取配置、不连接任何外部资源。

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

mod alerts;
mod database;
mod deploy;
mod keys;
mod operations;
mod settlement;
mod switches;
mod wal;
pub(crate) use alerts::AlertCommand;
pub(crate) use switches::SwitchCommand;
pub(crate) use wal::WalCommand;

pub(crate) use database::RebuildModeArg;
pub(crate) use database::{BackupCommand, DbCommand, OwnerSecretsArg};
pub(crate) use deploy::{DeployArgs, RollbackArgs};
pub(crate) use keys::KeysCommand;
pub(crate) use keys::KeysReencryptArgs;
pub(crate) use operations::{
    CredentialCommand, ForumCommand, FundsCommand, MailCommand, PmCommand, RecoveryCommand,
};
pub(crate) use operations::{
    CredentialKindArg, CredentialStatusArg, CredentialWorldArg, FundsWriteArgs, KeySourceArg,
};
pub(crate) use settlement::SettlementCommand;
pub(crate) use settlement::{ConfirmSentArgs, EquipmentLossArgs};

#[derive(Debug, Parser)]
#[command(
    name = "auction-cli",
    version,
    about = "Auction 运维命令行",
    long_about = "Auction 运维命令行：配置与部署预检、systemd 启停编排、数据库迁移/建库/备份恢复，\
以及凭据、规则、论坛、装备 PM、邮件证据、Bot 资金和结算异常等显式运维入口。\n\
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
    /// 数据库持久运行时开关，不重启角色。
    #[command(subcommand)]
    Switch(SwitchCommand),
    /// 单角色配置的安全投影与三角色部署一致性检查；不连接数据库。
    #[command(subcommand)]
    Config(ConfigCommand),
    /// 分体 systemd 部署的状态、顺序启动、停止与重启。
    Service(ServiceArgs),
    /// 从解压后的 CI 发布包制品部署：制品校验、时段门禁、演练回执、磁盘空间与在途发送门禁、
    /// 停机备份、新旧 CLI 校验、保留旧程序、安装、迁移、顺序启动与回执。只支持分体部署。
    Deploy(DeployArgs),
    /// 回退到 `deploy` 保留的旧程序；只用于不含迁移或全部登记为 compatible 的区间。
    Rollback(RollbackArgs),
    /// 数据库状态、迁移、建库、恢复预检、保留盘点与破坏性重建。
    #[command(subcommand)]
    Db(DbCommand),
    /// PostgreSQL custom-format 备份、离线校验与单事务恢复。
    #[command(subcommand)]
    Backup(BackupCommand),
    /// WAL 归档（archive_command 入口）与暂存上传（R2 独立前缀）。
    #[command(subcommand)]
    Wal(WalCommand),
    /// 主机侧运维告警：读取磁盘、备份回执与 WAL 暂存事实并写入告警来源。
    #[command(subcommand)]
    Alert(AlertCommand),
    /// 外部 Cookie 凭据的安全列表与 stdin 导入。
    #[command(subcommand)]
    Credential(CredentialCommand),
    /// 数据密钥环的只读状态与离线重加密。
    #[command(subcommand)]
    Keys(KeysCommand),
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
    /// 结算异常的只读预览与受审计人工处置。
    #[command(subcommand)]
    Settlement(SettlementCommand),
    /// 恢复后外部核对：暂停状态下只读扫描并把精确 Sent 匹配隔离为人工对账。
    #[command(subcommand)]
    Recovery(RecoveryCommand),
}

#[derive(Debug, Subcommand)]
pub(crate) enum ConfigCommand {
    /// 加载单个角色配置并输出不含任何秘密的有效配置投影。
    Show {
        /// 要加载的角色文件：`<config-dir>/<role>.toml`。
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
    /// 查询状态目录中的部署回执；只读文件，不取部署锁、不连接数据库。
    Receipt(ReceiptArgs),
    /// 严格预检、以迁移 owner 执行迁移，再按 Worker → Admin → Public 逐个启动并等待 readiness。
    Start {
        /// 每个角色等待 readiness 的秒数。
        #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(u64).range(1..=300))]
        startup_timeout_seconds: u64,
        #[command(flatten)]
        owner: OwnerSecretsArg,
        #[command(flatten)]
        receipt: ReceiptContextArgs,
    },
    /// 通过只读时段门禁后，按 Public → Admin → Worker 顺序停止三个 unit。
    Stop {
        #[command(flatten)]
        shutdown: ShutdownArgs,
    },
    /// 先预检新配置，再反序停止并执行与 start 相同的迁移和顺序启动。
    Restart {
        /// 每个角色等待 readiness 的秒数。
        #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(u64).range(1..=300))]
        startup_timeout_seconds: u64,
        /// 停机后执行迁移所用的 owner 凭据；服务账号没有 DDL 权限。
        #[command(flatten)]
        owner: OwnerSecretsArg,
        #[command(flatten)]
        shutdown: ShutdownArgs,
        #[command(flatten)]
        receipt: ReceiptContextArgs,
    },
}

/// 部署回执查询：`service receipt list/show`。
#[derive(Debug, Args)]
pub(crate) struct ReceiptArgs {
    #[command(subcommand)]
    pub(crate) command: ReceiptCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum ReceiptCommand {
    /// 按完成时间倒序列出状态目录中的部署回执。
    List {
        /// 最多返回的条数。
        #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u32).range(1..=200))]
        limit: u32,
    },
    /// 显示单条部署回执的完整内容。
    Show {
        /// 回执标识（JSON 中的 receipt_id）。
        receipt_id: String,
    },
}

/// `service start/restart` 写入部署回执的本次调用上下文；两项都可省略，缺省从环境推导操作者。
#[derive(Debug, Args)]
pub(crate) struct ReceiptContextArgs {
    /// 本次部署核对过的恢复点标识；仅写入回执，不触发备份或恢复。
    #[arg(long, value_name = "ID")]
    pub(crate) backup_id: Option<String>,
    /// 操作者名称；缺省依次取 SUDO_USER、USER、LOGNAME，均不可用时记为 `uid:<euid>`。
    #[arg(long, value_name = "NAME")]
    pub(crate) actor: Option<String>,
}

/// `deploy`/`rollback` 回执的操作者；恢复点标识由 deploy 的停机备份决定，rollback 回执为
/// null，因此不接受手填 `--backup-id`。
#[derive(Debug, Args)]
pub(crate) struct ActorArgs {
    /// 操作者名称；缺省依次取 SUDO_USER、USER、LOGNAME，均不可用时记为 `uid:<euid>`。
    #[arg(long, value_name = "NAME")]
    pub(crate) actor: Option<String>,
}

/// 只影响本次显式停机的时段保护与人工例外声明；不属于角色运行配置。
#[derive(Debug, Args)]
pub(crate) struct ShutdownArgs {
    /// lot 有效截止的保护窗口（分钟）；运维须按迁移演练停机时长的至少三倍设置。
    #[arg(long, default_value_t = 60, value_parser = clap::value_parser!(u32).range(1..=10080))]
    pub(crate) protection_window_minutes: u32,
    /// 绕过时段阻挡或查询失败；仍须提供 --reason，不绕过其他部署预检。
    #[arg(long, requires = "reason")]
    pub(crate) force: bool,
    /// 强制停机原因：1–1024 字节、首尾无空白且不含控制字符；不要填写秘密或玩家信息。
    #[arg(long, requires = "force", value_parser = shutdown_reason)]
    pub(crate) reason: Option<String>,
}

fn shutdown_reason(value: &str) -> Result<String, String> {
    if value.is_empty()
        || value.len() > 1024
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return Err("原因必须是 1–1024 字节、首尾无空白且不含控制字符的文本".to_owned());
    }
    Ok(value.to_owned())
}

/// 分体部署的宿主机边界；unit 名称只作为参数传入，不进入业务配置。
#[derive(Debug, Clone, Args)]
pub(crate) struct UnitArgs {
    /// systemctl 可执行文件的绝对路径。
    #[arg(long, value_name = "PATH", default_value = "/usr/bin/systemctl")]
    pub(crate) systemctl: PathBuf,
    /// 部署控制锁所在的私有状态目录；不存在时由 service 或分体维护命令以 0700 创建。
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

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, Parser};

    use super::{Cli, Command, ServiceAction};

    #[test]
    fn command_definitions_are_consistent() {
        Cli::command().debug_assert();
    }

    /// deploy/rollback 的恢复点标识不由运维手填：传入 --backup-id 是参数错误，只接受 --actor。
    #[test]
    fn deploy_and_rollback_accept_actor_but_not_backup_id() {
        let deploy =
            ["auction-cli", "deploy", "/srv/artifact", "--owner-secrets", "/etc/owner.toml"];
        let rollback = ["auction-cli", "rollback"];
        for base in [&deploy[..], &rollback[..]] {
            let mut arguments = base.to_vec();
            arguments.extend(["--backup-id", "backup-1"]);
            let error = Cli::try_parse_from(&arguments).expect_err("--backup-id 必须被拒绝");
            assert_eq!(error.kind(), clap::error::ErrorKind::UnknownArgument);
            let mut arguments = base.to_vec();
            arguments.extend(["--actor", "operator"]);
            Cli::try_parse_from(&arguments).expect("--actor 仍可用");
        }
    }

    /// service start/restart 仍可把人工核对的恢复点写入回执。
    #[test]
    fn service_start_still_accepts_backup_id() {
        let cli = Cli::try_parse_from([
            "auction-cli",
            "service",
            "start",
            "--owner-secrets",
            "/etc/owner.toml",
            "--backup-id",
            "backup-1",
        ])
        .expect("service start 接受 --backup-id");
        let Command::Service(service) = cli.command else { panic!("应解析为 service 命令") };
        let ServiceAction::Start { receipt, .. } = service.action else { panic!("应为 start") };
        assert_eq!(receipt.backup_id.as_deref(), Some("backup-1"));
    }

    /// `switch set --help` 是参数的权威说明：期限参数必须带说明与二选一约束。
    #[test]
    fn switch_set_help_documents_expiry() {
        let mut command = Cli::command();
        let set = command
            .find_subcommand_mut("switch")
            .and_then(|switch| switch.find_subcommand_mut("set"))
            .expect("switch set 子命令");
        let help = set.render_long_help().to_string();
        assert!(help.contains("--expires-at <RFC3339>"), "{help}");
        assert!(help.contains("与 --no-expiry 二选一"), "{help}");
        assert!(help.contains("switch list"), "{help}");
    }

    /// `deploy --help` 同样是权威说明：命令简介与 `--dry-run` 都要列出停机前的磁盘空间门禁，
    /// dry-run 还要说明会校验 --backup-dir 并输出 `gates.disk_space`。
    #[test]
    fn deploy_help_lists_disk_space_gate() {
        let command = Cli::command();
        let deploy = command.find_subcommand("deploy").expect("deploy 子命令");
        let about = deploy.get_about().map(ToString::to_string).unwrap_or_default();
        assert!(about.contains("磁盘空间与在途发送门禁"), "{about}");
        let dry_run = deploy
            .get_arguments()
            .find(|argument| argument.get_id().as_str() == "dry_run")
            .and_then(|argument| argument.get_help())
            .map(ToString::to_string)
            .expect("--dry-run 帮助");
        for gate in
            ["迁移版本与演练回执", "磁盘空间与在途发送门禁", "--backup-dir", "gates.disk_space"]
        {
            assert!(dry_run.contains(gate), "{dry_run}");
        }
    }
}
