//! 命令分派与稳定退出码；错误正文只包含已脱敏的上下文，调用方应依据退出码而非文本判断结果。

use clap::Parser;

use crate::cli::{Cli, Command, ConfigCommand};
use crate::deployment::ConfigDir;

mod output;

pub(crate) use output::{print_json, progress};

/// 成功。
const EXIT_OK: u8 = 0;
/// 配置加载或部署预检失败（clap 参数错误同样以 2 退出）。
const EXIT_CONFIG: u8 = 2;
/// 数据库、迁移、备份、恢复或重建失败。
const EXIT_DATABASE: u8 = 3;
/// systemd 启停编排失败或角色未就绪。
const EXIT_SERVICE: u8 = 4;
/// 业务运维命令失败、被拒绝，或已提交但需要人工处理（如资金冻结）。
const EXIT_OPERATION: u8 = 5;

/// 解析参数并执行命令；显式 `--help`/`--version` 由 clap 以 0 退出，非法参数以 2 退出。
pub(crate) async fn run() -> u8 {
    let cli = Cli::parse();
    let failure_code = failure_code(&cli.command);
    match dispatch(cli).await {
        Ok(()) => EXIT_OK,
        Err(error) => {
            eprintln!("auction-cli: {error:#}");
            failure_code
        }
    }
}

/// 退出码按命令类别分配，不按底层错误类型二次分类。
fn failure_code(command: &Command) -> u8 {
    match command {
        Command::Config(_) => EXIT_CONFIG,
        Command::Service(_) => EXIT_SERVICE,
        Command::Db(_) | Command::Backup(_) => EXIT_DATABASE,
        Command::Credential(_)
        | Command::Rules(_)
        | Command::Forum(_)
        | Command::Pm(_)
        | Command::Mail(_)
        | Command::Funds(_)
        | Command::Settlement(_) => EXIT_OPERATION,
    }
}

async fn dispatch(cli: Cli) -> anyhow::Result<()> {
    let config_dir = ConfigDir::new(&cli.config_dir)?;
    match cli.command {
        Command::Config(ConfigCommand::Show { role }) => {
            crate::deployment::show(&config_dir, role.role())
        }
        Command::Config(ConfigCommand::Check { strict }) => {
            let deployment = crate::deployment::check(&config_dir, strict)?;
            print_json(&deployment.report())
        }
        Command::Service(args) => crate::service::run(&config_dir, args).await,
        #[cfg(feature = "postgres")]
        Command::Db(command) => crate::database::run(&config_dir, command).await,
        #[cfg(feature = "postgres")]
        Command::Backup(command) => crate::backup::run(&config_dir, command).await,
        #[cfg(feature = "postgres")]
        Command::Credential(command) => {
            crate::operations::credentials::run(&config_dir, command).await
        }
        #[cfg(feature = "postgres")]
        Command::Rules(command) => crate::operations::rules::run(&config_dir, command).await,
        #[cfg(feature = "postgres")]
        Command::Forum(command) => crate::operations::forum::run(&config_dir, command).await,
        #[cfg(feature = "postgres")]
        Command::Pm(command) => crate::operations::watch_pm::run(&config_dir, command).await,
        #[cfg(feature = "postgres")]
        Command::Mail(command) => crate::operations::mail::run(&config_dir, command).await,
        #[cfg(feature = "postgres")]
        Command::Funds(command) => crate::operations::funds::run(&config_dir, command).await,
        #[cfg(feature = "postgres")]
        Command::Settlement(command) => {
            crate::operations::settlement::run(&config_dir, command).await
        }
        #[cfg(not(feature = "postgres"))]
        Command::Db(_)
        | Command::Backup(_)
        | Command::Credential(_)
        | Command::Rules(_)
        | Command::Forum(_)
        | Command::Pm(_)
        | Command::Mail(_)
        | Command::Funds(_)
        | Command::Settlement(_) => {
            anyhow::bail!("当前 auction-cli 未编译 postgres feature；数据库与业务运维命令不可用")
        }
    }
}
