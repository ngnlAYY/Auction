//! PostgreSQL 缺失目标数据库的显式、非破坏性 bootstrap 命令。

use super::super::*;

#[cfg(feature = "postgres")]
use auction_config::load_postgres_maintenance_secrets;
#[cfg(feature = "postgres")]
use auction_storage::{PostgresBootstrapResult, PostgresDatabaseBootstrap};

/// 在同一部署停机门禁内创建或幂等确认 PostgreSQL 应用数据库。
#[cfg(feature = "postgres")]
pub(crate) async fn postgres_bootstrap(
    controller: SystemdControllerArgs,
    confirmed_database_name: &str,
    maintenance_secrets_file: Option<&Path>,
) -> anyhow::Result<()> {
    let config = AppConfig::load(Role::Public)
        .context("load Public configuration for PostgreSQL bootstrap")?;
    let application_database_url = config
        .database_url()
        .context("Public database_url is required for PostgreSQL bootstrap")?;
    if !matches!(DatabaseBackend::from_url(application_database_url)?, DatabaseBackend::Postgres) {
        anyhow::bail!("postgres-bootstrap 只支持 PostgreSQL 完整档");
    }

    // 先锁定部署控制目录并确认三个角色已明确收敛；guard 存活至数据库后置连接验证结束。
    #[cfg(unix)]
    let _deployment_guard =
        deployment_control::acquire_migration_guard(deployment_control::ControllerOptions {
            systemctl: controller.systemctl,
            state_dir: controller.state_dir,
            worker_unit: controller.worker_unit,
            admin_unit: controller.admin_unit,
            public_unit: controller.public_unit,
        })?;
    #[cfg(not(unix))]
    {
        let _ = controller;
        anyhow::bail!("postgres-bootstrap 需要 Unix/systemd 角色运行状态检查")
    }
    let maintenance_secrets = maintenance_secrets_file
        .map(load_postgres_maintenance_secrets)
        .transpose()
        .context("load PostgreSQL maintenance sensitive TOML")?;
    let maintenance_database_url =
        crate::bootstrap::postgres_rebuild::resolve_maintenance_database_url(
            application_database_url,
            maintenance_secrets.as_ref().map(|secrets| secrets.database_url()),
            confirmed_database_name,
        )?;
    let owner = crate::bootstrap::postgres_rebuild::application_database_username(
        application_database_url,
        confirmed_database_name,
    )?;
    let mut bootstrap = PostgresDatabaseBootstrap::connect(
        &maintenance_database_url,
        confirmed_database_name,
        &owner,
    )
    .await
    .context("connect PostgreSQL maintenance database for bootstrap")?;
    let result = bootstrap
        .create_or_verify(application_database_url)
        .await
        .context("bootstrap PostgreSQL database")?;
    match result {
        PostgresBootstrapResult::Created { database_name, owner } => {
            println!(
                "postgres-bootstrap created database={} owner={} migrations=not_run",
                database_name, owner
            );
        }
        PostgresBootstrapResult::AlreadyExists { database_name, owner } => {
            println!(
                "postgres-bootstrap already_exists database={} owner={} migrations=not_run",
                database_name, owner
            );
        }
    }
    Ok(())
}

#[cfg(not(feature = "postgres"))]
pub(crate) async fn postgres_bootstrap(
    _controller: SystemdControllerArgs,
    _confirmed_database_name: &str,
    _maintenance_secrets_file: Option<&Path>,
) -> anyhow::Result<()> {
    anyhow::bail!("postgres-bootstrap 需要 PostgreSQL 完整档；请使用 --features postgres")
}
