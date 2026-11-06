//! 制品部署与回退命令参数；不读取配置、不校验制品、不连接任何外部资源。

use std::path::PathBuf;

use clap::Args;

use super::{OwnerSecretsArg, ReceiptContextArgs, ShutdownArgs, UnitArgs};

/// 分体制品部署：`auction-cli deploy <制品目录>`。
#[derive(Debug, Args)]
pub(crate) struct DeployArgs {
    /// 解压后的 CI 发布包根目录；必须含 bin/、RELEASE-MANIFEST.v1、SHA256SUMS 与 docs/ Web 标记。
    pub(crate) artifact_dir: PathBuf,
    #[command(flatten)]
    pub(crate) units: UnitArgs,
    /// 停机后执行迁移所用的 owner 凭据；服务账号没有 DDL 权限。
    #[command(flatten)]
    pub(crate) owner: OwnerSecretsArg,
    #[command(flatten)]
    pub(crate) shutdown: ShutdownArgs,
    #[command(flatten)]
    pub(crate) receipt: ReceiptContextArgs,
    /// 程序安装目录；缺省取当前运行三角色 MainPID 可执行文件所在目录，必须三者一致。
    #[arg(long, value_name = "DIR")]
    pub(crate) install_dir: Option<PathBuf>,
    /// 停机备份的绝对父目录；archive 与 manifest 都以新的不可覆盖文件名写入。
    #[arg(long, value_name = "DIR", default_value = "/var/backups/auction")]
    pub(crate) backup_dir: PathBuf,
    /// 含迁移版本必须提供的 W10.6 迁移演练回执（`auction-migration-drill.v1`）。
    #[arg(long, value_name = "FILE")]
    pub(crate) migration_drill_receipt: Option<PathBuf>,
    /// 演练回执的最大年龄（天）；超龄回执不能证明本次迁移区间。
    #[arg(long, default_value_t = 7, value_parser = clap::value_parser!(u32).range(1..=30))]
    pub(crate) drill_max_age_days: u32,
    /// 在途发送门禁等待已越过 sending 围栏的投递完成的秒数；超时即拒绝部署。
    #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(u64).range(0..=600))]
    pub(crate) send_gate_timeout_seconds: u64,
    /// 每个角色启动后等待 readiness 的秒数。
    #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(u64).range(1..=300))]
    pub(crate) startup_timeout_seconds: u64,
    /// 只执行制品、配置、时段、在途发送与版本门禁并输出计划；不备份、不停机、不安装、不写回执。
    #[arg(long)]
    pub(crate) dry_run: bool,
}

/// 回退到某次 deploy 保留的旧程序；只允许不含迁移或全部登记为 compatible 的区间。
#[derive(Debug, Args)]
pub(crate) struct RollbackArgs {
    /// 目标回滚记录的 deploy 回执 UUID；缺省使用最新一条。
    #[arg(long, value_name = "RECEIPT_ID")]
    pub(crate) receipt_id: Option<String>,
    #[command(flatten)]
    pub(crate) units: UnitArgs,
    #[command(flatten)]
    pub(crate) shutdown: ShutdownArgs,
    #[command(flatten)]
    pub(crate) receipt: ReceiptContextArgs,
    /// 程序安装目录；缺省取当前运行三角色 MainPID 可执行文件所在目录。
    #[arg(long, value_name = "DIR")]
    pub(crate) install_dir: Option<PathBuf>,
    /// 在途发送门禁等待秒数；超时即拒绝回退。
    #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(u64).range(0..=600))]
    pub(crate) send_gate_timeout_seconds: u64,
    /// 每个角色启动后等待 readiness 的秒数。
    #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(u64).range(1..=300))]
    pub(crate) startup_timeout_seconds: u64,
    /// 只执行回滚记录、兼容区间与门禁核对；不停机、不安装、不写回执。
    #[arg(long)]
    pub(crate) dry_run: bool,
}
