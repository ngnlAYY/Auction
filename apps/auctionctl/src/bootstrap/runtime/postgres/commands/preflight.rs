//! PostgreSQL 只读 retention/recovery 预检命令。

use super::super::*;

#[cfg(feature = "postgres")]
pub(crate) async fn retention_inventory(
    output: &Path,
    confirmed_database_name: &str,
) -> anyhow::Result<()> {
    validate_private_evidence_path(output)?;
    let store = connect_postgres(
        "database_url is required for retention inventory",
        "retention-inventory 只支持 PostgreSQL 完整档",
        "connect database for retention inventory",
    )
    .await?;
    let inventory = match store {
        DatabaseStore::Postgres(postgres) => postgres
            .collect_retention_inventory(confirmed_database_name)
            .await
            .context("collect PostgreSQL retention inventory")?,
        #[cfg(feature = "sqlite")]
        DatabaseStore::Sqlite(_) => anyhow::bail!("retention-inventory 只支持 PostgreSQL 完整档"),
    };
    let evidence =
        serde_json::to_vec_pretty(&inventory).context("encode retention inventory evidence")?;
    write_private_evidence(output, &evidence)?;
    println!(
        "retention-inventory written path={} database={} relations={} policy_tables={} policy_sha256={} migration_current={}",
        output.display(),
        inventory.database_name,
        inventory.relation_count,
        inventory.policy_table_count,
        inventory.policy_sha256,
        inventory.migration.current,
    );
    Ok(())
}

#[cfg(feature = "postgres")]
pub(crate) async fn recovery_preflight(
    output: &Path,
    confirmed_database_name: &str,
) -> anyhow::Result<()> {
    validate_private_evidence_path(output)?;
    let store = connect_postgres(
        "database_url is required for recovery preflight",
        "recovery-preflight 只支持 PostgreSQL 完整档",
        "connect database for recovery preflight",
    )
    .await?;
    let preflight = match store {
        DatabaseStore::Postgres(postgres) => postgres
            .collect_recovery_preflight(confirmed_database_name)
            .await
            .context("collect PostgreSQL recovery preflight")?,
        #[cfg(feature = "sqlite")]
        DatabaseStore::Sqlite(_) => anyhow::bail!("recovery-preflight 只支持 PostgreSQL 完整档"),
    };
    let evidence =
        serde_json::to_vec_pretty(&preflight).context("encode recovery preflight evidence")?;
    write_private_evidence(output, &evidence)?;
    println!(
        "recovery-preflight written path={} database={} passed={} active_sessions={} user_objects={}",
        output.display(),
        preflight.database_name,
        preflight.restore_preflight_passed,
        preflight.active_other_sessions,
        preflight.user_object_count,
    );
    Ok(())
}

#[cfg(not(feature = "postgres"))]
pub(crate) async fn recovery_preflight(
    _output: &Path,
    _confirmed_database_name: &str,
) -> anyhow::Result<()> {
    Err(postgres_feature_required("recovery-preflight"))
}

#[cfg(not(feature = "postgres"))]
pub(crate) async fn retention_inventory(
    _output: &Path,
    _confirmed_database_name: &str,
) -> anyhow::Result<()> {
    Err(postgres_feature_required("retention-inventory"))
}

#[cfg(feature = "postgres")]
async fn connect_postgres(
    database_url_context: &'static str,
    unsupported_context: &'static str,
    connect_context: &'static str,
) -> anyhow::Result<DatabaseStore> {
    let config = AppConfig::load(Role::Worker).context("load worker configuration")?;
    let database_url = config.database_url().context(database_url_context)?;
    if !matches!(DatabaseBackend::from_url(database_url)?, DatabaseBackend::Postgres) {
        anyhow::bail!("{unsupported_context}");
    }
    DatabaseStore::connect(database_url).await.context(connect_context)
}

#[cfg(not(feature = "postgres"))]
fn postgres_feature_required(command: &str) -> anyhow::Error {
    anyhow::anyhow!("{command} 需要 PostgreSQL 完整档；请使用 --features postgres")
}
