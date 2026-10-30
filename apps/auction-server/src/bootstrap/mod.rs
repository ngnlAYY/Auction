//! 单文件首次配置与角色启动的装配；业务状态机仍属于既有 crate。

mod cli;
mod configuration;
mod filesystem;
mod probe;
mod supervisor;

use anyhow::{Context, bail};
use clap::Parser;
use cli::{ChildRole, Cli};

pub(super) async fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
    if let Some(role) = cli.role {
        let path = cli.config_file.context("child role requires an explicit config path")?;
        if !path.is_absolute() {
            bail!("child config path must be absolute");
        }
        return match role {
            ChildRole::Worker => auction_worker::run_from_path(path).await,
            ChildRole::Admin => auction_admin_api::run_from_path(path).await,
            ChildRole::Public => auction_public_api::run_from_path(path).await,
        };
    }
    if std::env::var_os("AUCTION_CONFIG_FILE").is_some() {
        bail!("auction-server uses --config-dir, not AUCTION_CONFIG_FILE");
    }
    let config_dir = filesystem::config_path(&cli.config_dir)?;
    let runtime = auction_config::managed_runtime_directory(&config_dir)?;
    let ownership = filesystem::acquire_runtime(&runtime)?;
    let mut signals = auction_process::TerminationSignals::subscribe()?;
    let plans = tokio::select! {
        result = prepare(&cli, &config_dir) => result?,
        signal = signals.recv() => { signal?; bail!("startup interrupted; inspect saved configuration before restarting"); }
    };
    supervisor::run(
        plans,
        ownership,
        signals,
        std::time::Duration::from_secs(cli.startup_timeout_seconds),
    )
    .await
}

async fn prepare(
    cli: &Cli,
    config_dir: &std::path::Path,
) -> anyhow::Result<Vec<configuration::RolePlan>> {
    let first_run = match std::fs::symlink_metadata(config_dir) {
        Ok(_) => {
            filesystem::trusted_directory(config_dir)?;
            false
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
        Err(error) => return Err(error).context("inspect server configuration directory"),
    };
    if first_run {
        setup(cli, config_dir).await?;
    }
    let plans = configuration::load(config_dir)?;
    configuration::ensure_available(&plans).await?;
    if first_run {
        eprintln!("配置已保存；正在初始化专用空 PostgreSQL 数据库，尚未启动角色。");
        let url = plans[0].config.database_url().context("database configuration is required")?;
        tokio::time::timeout(
            std::time::Duration::from_secs(300),
            auction_storage::initialize_empty_postgres(url),
        )
        .await
        .context("首次初始化超时；保留配置与数据库现场，普通重启不会重试迁移")?
        .context("首次初始化失败；保留配置，核对数据库后用 auction-cli 的 db 命令受控恢复")?;
    }
    // 空库、部分迁移或待升级 schema 不允许先启动 Worker；只读就绪校验不能隐式迁移。
    use auction_application::ProcessReadinessPort;
    let url = plans[0].config.database_url().context("database configuration is required")?;
    let ready = async {
        let store = auction_storage::DatabaseStore::connect(url).await?;
        let result = store.check().await;
        store.postgres_pool().close().await;
        result.map_err(anyhow::Error::from)
    };
    tokio::time::timeout(std::time::Duration::from_secs(30), ready).await
        .context("database readiness timeout")?
        .context("数据库 schema 未就绪；停止 auction-server 后以同一服务账号执行 auction-cli --config-dir <配置目录> db migrate")?;
    Ok(plans)
}

async fn setup(cli: &Cli, output: &std::path::Path) -> anyhow::Result<()> {
    if !cfg!(feature = "web-embed") {
        bail!("first-run setup requires a web-embed release");
    }
    let remote = if cli.allow_remote_setup {
        Some(auction_setup::RemoteSetupAccess {
            origin: cli.setup_origin.clone().context("remote setup requires --setup-origin")?,
            proxy_ip: cli.setup_proxy_ip.context("remote setup requires --setup-proxy-ip")?,
        })
    } else {
        None
    };
    let outcome = auction_setup::run_first_run_with_policy::<auction_admin_api::AdminWebAssets>(
        auction_setup::FirstRunServerOptions {
            output_dir: output.to_owned(),
            control_group: filesystem::current_group()?,
            // SAFETY：只读取当前服务账号有效 UID，用于本机 control allowlist。
            control_uid: unsafe { libc::geteuid() },
        },
        auction_setup::FirstRunPolicy {
            listen: cli.setup_listen,
            remote,
            initialize_and_start: true,
        },
    )
    .await
    .map_err(anyhow::Error::msg)?;
    match outcome {
        auction_setup::FirstRunOutcome::Completed => Ok(()),
        auction_setup::FirstRunOutcome::Interrupted => {
            bail!("first-run setup interrupted; no services started")
        }
        auction_setup::FirstRunOutcome::OutcomeUnknown => {
            bail!("first-run outcome unknown; inspect configuration before restarting")
        }
    }
}
