//! 单文件首次配置与角色启动的装配；业务状态机仍属于既有 crate。

mod cli;
mod configuration;
mod filesystem;
mod supervisor;

use std::{future::Future, ops::AsyncFnOnce, path::Path};

use anyhow::{Context, bail};
use auction_setup::{FirstRunOutcome, SetupDatabaseTarget};
use clap::Parser;

use cli::{ChildRole, Cli};

/// 进程入口：先解析参数，再按身份选择 runtime。supervisor 只等待、转发信号与回收子进程，
/// `--role` 子角色承载真实业务负载。
pub(super) fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    runtime_builder(cli.role)
        .enable_all()
        .build()
        .context("构建 server 运行时失败")?
        .block_on(run(cli))
}

/// supervisor 保持单线程：子进程由主线程派生，`PR_SET_PDEATHSIG` 绑定派生线程，改成多线程后
/// 父死信号会绑定到某个工作线程而非整个进程的生命周期。子角色与分体部署的三个程序一致，使用
/// 多线程 runtime，避免 HTML 解析、摘要等 CPU 段阻塞同进程的心跳、control 就绪与推送。
fn runtime_builder(role: Option<ChildRole>) -> tokio::runtime::Builder {
    match role {
        Some(_) => tokio::runtime::Builder::new_multi_thread(),
        None => tokio::runtime::Builder::new_current_thread(),
    }
}

async fn run(cli: Cli) -> anyhow::Result<()> {
    if let Some(role) = cli.role {
        let path = cli.config_file.context("子角色必须显式传入配置路径")?;
        if !path.is_absolute() {
            bail!("子角色配置路径必须是绝对路径");
        }
        return match role {
            ChildRole::Worker => auction_worker::run_from_path(path).await,
            ChildRole::Admin => auction_admin_api::run_from_path(path).await,
            ChildRole::Public => auction_public_api::run_from_path(path).await,
        };
    }
    // 子角色在自己的配置日志初始化后安装；父进程只开启安全 panic target。
    auction_observability::init_json_stderr("off,auction::panic=error")
        .context("初始化 server 日志失败")?;
    auction_observability::install_safe_panic_hook();
    if std::env::var_os("AUCTION_CONFIG_FILE").is_some() {
        bail!("auction-server 使用 --config-dir，不读取 AUCTION_CONFIG_FILE");
    }
    let config_dir = filesystem::config_path(&cli.config_dir)?;
    let runtime = auction_config::managed_runtime_directory(&config_dir)?;
    let ownership = filesystem::acquire_runtime(&runtime)?;
    let mut signals = auction_process::TerminationSignals::subscribe()?;
    let plans = tokio::select! {
        result = prepare(&cli, &config_dir) => result?,
        signal = signals.recv() => { signal?; bail!("启动被中断；重启前先核对已保存的配置"); }
    };
    supervisor::run(
        plans,
        ownership,
        signals,
        std::time::Duration::from_secs(cli.startup_timeout_seconds),
    )
    .await
}

async fn prepare(cli: &Cli, config_dir: &Path) -> anyhow::Result<Vec<configuration::RolePlan>> {
    let plans = prepare_configuration(
        cli,
        config_dir,
        setup(cli, config_dir),
        configuration::ensure_available,
        auction_storage::initialize_empty_postgres,
    )
    .await?;
    // 空库、部分迁移或待升级 schema 不允许先启动 Worker；只读就绪校验不能隐式迁移。
    use auction_application::operations::ProcessReadinessPort;
    let url = plans[0].config.database_url().context("缺少数据库配置")?;
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
        .context("数据库就绪检查超时")?
        .context("数据库 schema 未就绪；停止 auction-server 后以同一服务账号执行 auction-cli --config-dir <配置目录> db migrate")?;
    Ok(plans)
}

async fn prepare_configuration(
    cli: &Cli,
    config_dir: &Path,
    setup_outcome: impl Future<Output = anyhow::Result<FirstRunOutcome>>,
    ensure_available: impl AsyncFnOnce(&[configuration::RolePlan]) -> anyhow::Result<()>,
    initialize: impl AsyncFnOnce(
        &str,
        auction_storage::DatabaseConnectionProfile,
    ) -> Result<(), auction_storage::FirstDatabaseInitializationError>,
) -> anyhow::Result<Vec<configuration::RolePlan>> {
    let first_run = match std::fs::symlink_metadata(config_dir) {
        Ok(_) => {
            filesystem::trusted_directory(config_dir)?;
            false
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
        Err(error) => return Err(error).context("检查 server 配置目录失败"),
    };
    let confirmed_database = if first_run {
        // 严格配置的服务账号固定为无 DDL 的 auction_runtime，不能执行首次 CREATE SCHEMA/迁移。
        if cli.owner_secrets.is_none() {
            bail!(
                "首次设置必须提供 --owner-secrets（迁移 owner 凭据）；服务账号 auction_runtime 没有 DDL 权限"
            );
        }
        match setup_outcome.await? {
            FirstRunOutcome::Completed { confirmed_database } => Some(
                confirmed_database
                    .context("首次设置缺少已确认的数据库目标，请核对配置与迁移 owner 凭据")?,
            ),
            FirstRunOutcome::Interrupted => bail!("首次设置被中断；未启动任何角色"),
            FirstRunOutcome::OutcomeUnknown => bail!("首次设置结果未知；重启前先核对配置"),
        }
    } else {
        None
    };
    let plans = configuration::load(config_dir)?;
    ensure_available(&plans).await?;
    if let Some(confirmed_database) = confirmed_database {
        let owner_path = cli.owner_secrets.as_deref().context("首次设置缺少迁移 owner 凭据")?;
        let owner = auction_config::load_postgres_owner_secrets(owner_path)
            .context("读取迁移 owner 凭据失败；不会回退到服务账号")?;
        let service_url = plans[0].config.database_url().context("缺少数据库配置")?;
        same_database_target(&confirmed_database, service_url, owner.database_url())?;
        eprintln!("配置已保存；正在初始化专用空 PostgreSQL 数据库，尚未启动角色。");
        tokio::time::timeout(
            std::time::Duration::from_secs(300),
            initialize(
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
    Ok(plans)
}

async fn setup(cli: &Cli, output: &Path) -> anyhow::Result<FirstRunOutcome> {
    if !cfg!(feature = "web-embed") {
        bail!("首次设置需要启用 web-embed 的发布构建");
    }
    let remote = if cli.allow_remote_setup {
        Some(auction_setup::RemoteSetupAccess {
            origin: cli.setup_origin.clone().context("远程设置需要 --setup-origin")?,
            proxy_ip: cli.setup_proxy_ip.context("远程设置需要 --setup-proxy-ip")?,
        })
    } else {
        None
    };
    auction_setup::run_first_run_with_policy::<auction_admin_api::AdminWebAssets>(
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
        auction_setup::FirstRunPolicy::DIOXUS_WEB_SCRIPT_POLICY,
    )
    .await
    .map_err(anyhow::Error::msg)
}

/// 初始化只授权给确认时的目标，服务配置和实际用于迁移的 owner 凭据都要复核。
fn same_database_target(
    confirmed: &SetupDatabaseTarget,
    service_url: &str,
    owner_url: &str,
) -> anyhow::Result<()> {
    if SetupDatabaseTarget::from_database_url(service_url).as_ref() != Some(confirmed)
        || SetupDatabaseTarget::from_database_url(owner_url).as_ref() != Some(confirmed)
    {
        bail!(
            "重新读取的数据库目标与首次设置确认不一致，已拒绝初始化。请核对服务配置与迁移 owner 凭据"
        );
    }
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

#[cfg(test)]
mod tests {
    use std::{
        cell::Cell,
        ffi::OsStr,
        fs,
        io::Write,
        os::unix::fs::{DirBuilderExt, OpenOptionsExt},
        path::PathBuf,
        sync::atomic::{AtomicUsize, Ordering},
    };

    use auction_config::{AuctionScheduleConfig, FirstRunConfig, PostgresConnectionConfig};
    use auction_setup::{FirstRunOutcome, SetupDatabaseTarget};
    use clap::Parser;
    use tokio::runtime::RuntimeFlavor;

    use super::cli::{ChildRole, Cli};
    use super::configuration::RolePlan;
    use super::{filesystem, prepare_configuration, runtime_builder};

    #[test]
    fn child_roles_run_multi_threaded_while_supervisor_stays_single_threaded() {
        for role in ChildRole::ORDER {
            let runtime = runtime_builder(Some(role)).enable_all().build().unwrap();
            assert_eq!(runtime.handle().runtime_flavor(), RuntimeFlavor::MultiThread);
        }
        let supervisor = runtime_builder(None).enable_all().build().unwrap();
        assert_eq!(supervisor.handle().runtime_flavor(), RuntimeFlavor::CurrentThread);
    }

    #[tokio::test]
    async fn rejects_database_changed_after_setup_confirmation() {
        let calls = Cell::new(0);
        let result = prepare_after_setup("replaced", &calls).await;
        assert_eq!(calls.get(), 0);
        let error = result.expect_err("首次启动必须拒绝重新指向的数据库").to_string();
        assert!(error.contains("首次设置确认不一致"), "{error}");
        assert!(!error.contains("fixture-password"));
    }

    #[tokio::test]
    async fn initializes_the_confirmed_database_once() {
        let calls = Cell::new(0);
        prepare_after_setup("confirmed", &calls).await.unwrap();
        assert_eq!(calls.get(), 1);
    }

    async fn prepare_after_setup(
        reloaded_database: &str,
        calls: &Cell<usize>,
    ) -> anyhow::Result<()> {
        let directory = StartupDirectory::new();
        let config_dir = directory.0.join("config");
        let owner_path = directory.0.join("owner.toml");
        let cli = Cli::parse_from([
            OsStr::new("auction-server"),
            OsStr::new("--owner-secrets"),
            owner_path.as_os_str(),
        ]);
        let runtime = auction_config::managed_runtime_directory(&config_dir)?;
        let _ownership = filesystem::acquire_runtime(&runtime)?;
        let setup_outcome = async {
            let mut config = first_run_config();
            config.write_managed_to(&config_dir)?;
            let confirmed_database =
                SetupDatabaseTarget::from_database_url(&config.database.connection_url()?);
            let outcome = FirstRunOutcome::Completed { confirmed_database };

            // 在发布成功后改写本机配置，保留刚刚完成的确认结果。
            if config.database.name != reloaded_database {
                fs::rename(&config_dir, directory.0.join("published-config"))?;
                config.database.name = reloaded_database.to_owned();
                config.write_managed_to(&config_dir)?;
            }
            let mut owner = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&owner_path)?;
            writeln!(
                owner,
                "database_url = \"postgresql://auction_owner:fixture-password@127.0.0.1:5432/{reloaded_database}\""
            )?;
            Ok(outcome)
        };
        prepare_configuration(
            &cli,
            &config_dir,
            setup_outcome,
            async |_: &[RolePlan]| Ok(()),
            async |owner_url: &str, _profile| {
                calls.set(calls.get() + 1);
                let target = auction_storage::PostgresTarget::parse(owner_url).unwrap();
                assert_eq!(target.database, reloaded_database);
                assert_eq!(target.username, "auction_owner");
                Ok(())
            },
        )
        .await
        .map(|_plans| ())
    }

    fn first_run_config() -> FirstRunConfig {
        FirstRunConfig {
            database: PostgresConnectionConfig {
                host: "127.0.0.1".to_owned(),
                port: 5432,
                username: "auction_runtime".to_owned(),
                password: "fixture-password".to_owned(),
                name: "confirmed".to_owned(),
            },
            admin_actor: "operator".to_owned(),
            admin_secret: "a".repeat(32),
            public_origin: "https://public.example.test".to_owned(),
            admin_origin: "https://admin.example.test".to_owned(),
            public_listener_port: 8090,
            admin_listener_port: 8091,
            metrics_enabled: true,
            schedule: AuctionScheduleConfig::default(),
            start_price: 1000,
            log_filter: "info".to_owned(),
            worker_concurrency: 4,
            worker_poll_interval_seconds: 15,
            realtime_poll_interval_millis: 250,
            control_group: "auction".to_owned(),
            // SAFETY：测试用配置的控制面权限与当前有效 UID 对齐。
            control_uid: unsafe { libc::geteuid() },
        }
    }

    struct StartupDirectory(PathBuf);

    impl StartupDirectory {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().canonicalize().unwrap().join(format!(
                "auction-startup-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed),
            ));
            fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for StartupDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
