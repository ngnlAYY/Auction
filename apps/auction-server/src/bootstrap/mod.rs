//! 单文件首次配置与角色启动的装配；业务状态机仍属于既有 crate。

mod cli;
mod configuration;
mod filesystem;
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
    // 子角色在自己的配置日志初始化后安装；父进程只开启安全 panic target。
    auction_observability::init_json_stderr("off,auction::panic=error")
        .context("initialize server tracing")?;
    auction_observability::install_safe_panic_hook();
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
        // 服务账号在严格配置中固定为无 DDL 的 auction_runtime；首次空库迁移必须使用
        // 独立 owner 凭据，不能再用服务连接执行 CREATE SCHEMA/迁移。
        if cli.owner_secrets.is_none() {
            bail!(
                "首次设置必须提供 --owner-secrets（迁移 owner 凭据）；服务账号 auction_runtime 没有 DDL 权限"
            );
        }
        setup(cli, config_dir).await?;
    }
    let plans = configuration::load(config_dir)?;
    configuration::ensure_available(&plans).await?;
    if first_run {
        eprintln!("配置已保存；正在初始化专用空 PostgreSQL 数据库，尚未启动角色。");
        let owner_path = cli.owner_secrets.as_deref().context("首次设置缺少迁移 owner 凭据")?;
        let owner = auction_config::load_postgres_owner_secrets(owner_path)
            .context("读取迁移 owner 凭据失败；不会回退到服务账号")?;
        let service_url =
            plans[0].config.database_url().context("database configuration is required")?;
        same_database_target(service_url, owner.database_url())?;
        tokio::time::timeout(
            std::time::Duration::from_secs(300),
            auction_storage::initialize_empty_postgres(
                owner.database_url(),
                database_profile(
                    &plans[0].config,
                    auction_storage::DatabaseConnectionPurpose::ServerInitialize,
                ),
            ),
        )
        .await
        .context("首次初始化超时；保留配置与数据库现场，普通重启不会重试迁移")?
        .context("首次初始化失败；保留配置，核对数据库后用 auction-cli 的 db 命令受控恢复")?;
    }
    // 空库、部分迁移或待升级 schema 不允许先启动 Worker；只读就绪校验不能隐式迁移。
    use auction_application::operations::ProcessReadinessPort;
    let url = plans[0].config.database_url().context("database configuration is required")?;
    let ready = async {
        let store = auction_storage::DatabaseStore::connect(
            url,
            database_profile(
                &plans[0].config,
                auction_storage::DatabaseConnectionPurpose::ServerPreflight,
            ),
        )
        .await?
        .with_audit_failure_mode(plans[0].config.readiness.audit_failure_mode)
        .with_migration_compatibility_mode(plans[0].config.readiness.migration_compatibility_mode);
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

/// 证明迁移 owner 与服务配置指向同一数据库端点；错误不回显连接串或密码。
///
/// 复用 SQLx 的实际解析选项，覆盖 host/hostaddr、Unix socket、显式端口与查询参数，
/// 不用 `Url` 的 host/path 近似判断。
fn same_database_target(service_url: &str, owner_url: &str) -> anyhow::Result<()> {
    let service =
        auction_storage::PostgresTarget::parse(service_url).ok().context("服务数据库连接无效")?;
    let owner = auction_storage::PostgresTarget::parse(owner_url)
        .ok()
        .context("迁移 owner 数据库连接无效")?;
    service
        .require_same_database(&owner)
        .ok()
        .context("迁移 owner 凭据必须指向服务配置的同一主机、端口与数据库")?;
    if service.username == owner.username {
        bail!("迁移 owner 账号不能与服务运行账号相同");
    }
    Ok(())
}

fn database_profile(
    config: &auction_config::AppConfig,
    purpose: auction_storage::DatabaseConnectionPurpose,
) -> auction_storage::DatabaseConnectionProfile {
    if config.database_profiles.mode == auction_config::DatabaseProfileMode::Legacy {
        return auction_storage::DatabaseConnectionProfile::legacy();
    }
    let maintenance = config.database_profiles.maintenance;
    auction_storage::DatabaseConnectionProfile::maintenance(
        purpose,
        maintenance.max_connections,
        maintenance.acquire_timeout_millis,
    )
}
