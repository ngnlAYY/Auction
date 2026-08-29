//! PostgreSQL v28 现有数据接管的只读预检、二次证据校验与写入编排。

use std::path::Path;

use anyhow::Context;
use auction_config::{AppConfig, Role};
use auction_storage::DatabaseStore;

use super::super::{PostgresCommandEnvironment, sha256_text};
use super::common::{
    preflight_sha256, require_adopted_schema, require_adoption_ready, require_postgres_backend,
    require_same_backup,
};
use super::model::{LEGACY_V28_ADOPTION_RECEIPT_FORMAT, LegacyV28AdoptionReceipt};
use super::receipt::AdoptionReceiptReservation;
use super::verification::verify_backup;

/// 默认只读演练；只有所有外部证据齐备并逐字确认后才执行接管。
pub(in crate::bootstrap) async fn adopt(
    confirmed_database_name: &str,
    backup_manifest: Option<&Path>,
    execute: bool,
    confirm_adoption: Option<&str>,
    receipt_path: Option<&Path>,
) -> anyhow::Result<()> {
    if !execute && (confirm_adoption.is_some() || receipt_path.is_some()) {
        anyhow::bail!("postgres-adopt-legacy-v28 dry-run 不接受 --confirm-adoption 或 --receipt");
    }
    let verified =
        backup_manifest.map(|path| verify_backup(path, confirmed_database_name)).transpose()?;
    if execute {
        let expected_confirmation = format!("ADOPT LEGACY V28 {confirmed_database_name}");
        if confirm_adoption != Some(expected_confirmation.as_str()) {
            anyhow::bail!("actual adoption requires --confirm-adoption '{expected_confirmation}'");
        }
        if verified.is_none() {
            anyhow::bail!("actual adoption requires --backup-manifest");
        }
        if receipt_path.is_none() {
            anyhow::bail!("actual adoption requires --receipt");
        }
    }

    let config = AppConfig::load(Role::Worker).context("load worker configuration")?;
    let database_url =
        config.database_url().context("database_url is required for legacy v28 adoption")?;
    require_postgres_backend(database_url)?;
    // 同时解析外部工具连接环境，确保配置中的数据库路径也精确匹配人工确认；本命令不会
    // 调用外部工具，但沿用 backup 的严格 libpq URL 边界可以避免两个命令解释目标不同。
    let _command_environment =
        PostgresCommandEnvironment::from_database_url(database_url, confirmed_database_name)?;
    let store = DatabaseStore::connect(database_url)
        .await
        .context("connect database for legacy v28 adoption")?;
    let postgres = match store {
        DatabaseStore::Postgres(postgres) => postgres,
        #[cfg(feature = "sqlite")]
        DatabaseStore::Sqlite(_) => {
            anyhow::bail!("postgres-adopt-legacy-v28 只支持 PostgreSQL 完整档")
        }
    };
    let preflight = postgres
        .collect_legacy_v28_preflight(confirmed_database_name)
        .await
        .context("collect legacy v28 adoption preflight")?;
    require_adoption_ready(&preflight)?;
    let current_preflight_sha256 = preflight_sha256(&preflight)?;

    if !execute {
        if let Some(verified) = &verified
            && verified.manifest.preflight_sha256 != current_preflight_sha256
        {
            anyhow::bail!(
                "live legacy v28 source shape does not match the supplied backup manifest"
            );
        }
        postgres.close().await;
        println!(
            "postgres-adopt-legacy-v28 dry-run passed database={} legacy_version={} tables={} source_shape_issues={} policy_issues={} lot_projection_issues={} bid_event_issues={} backup_verified={}",
            confirmed_database_name,
            preflight.legacy_version.unwrap_or_default(),
            preflight.actual_table_count,
            preflight.source_shape_issue_count,
            preflight.policy_issue_count,
            preflight.lot_projection_issue_count,
            preflight.bid_event_issue_count,
            verified.is_some(),
        );
        return Ok(());
    }

    let receipt_path = receipt_path
        .ok_or_else(|| anyhow::anyhow!("required adoption receipt path disappeared"))?;
    let mut receipt = Some(AdoptionReceiptReservation::reserve(receipt_path)?);
    let verified = verified
        .ok_or_else(|| anyhow::anyhow!("required legacy v28 backup evidence disappeared"))?;
    let manifest_path = backup_manifest
        .ok_or_else(|| anyhow::anyhow!("required backup manifest path disappeared"))?;

    // 数据库 mutation 前重新读取两个 artifact，拒绝预检期间发生的替换或内容变化。
    let reverified = verify_backup(manifest_path, confirmed_database_name)?;
    require_same_backup(&verified, &reverified)?;
    if reverified.manifest.preflight_sha256 != current_preflight_sha256 {
        anyhow::bail!("live legacy v28 source shape does not match the verified backup manifest");
    }

    let adoption = postgres
        .adopt_legacy_v28(confirmed_database_name)
        .await
        .context("adopt canonical Auction-BF PostgreSQL v28")?;
    let postflight = postgres
        .collect_recovery_preflight(confirmed_database_name)
        .await
        .context("legacy v28 adoption committed but postflight collection failed")?;
    require_adopted_schema(&postflight, adoption.final_sqlx_version)?;
    postgres.close().await;

    let receipt_payload = LegacyV28AdoptionReceipt {
        format: LEGACY_V28_ADOPTION_RECEIPT_FORMAT,
        adopted_at: adoption.adopted_at,
        database_name_sha256: sha256_text(confirmed_database_name),
        archive_file_name: reverified.manifest.archive_file_name,
        archive_sha256: reverified.manifest.archive_sha256,
        manifest_sha256: reverified.manifest_sha256,
        preflight_sha256: reverified.manifest.preflight_sha256,
        legacy_migration_version: adoption.legacy_version,
        baselined_sqlx_versions: adoption.baselined_sqlx_versions,
        final_sqlx_version: adoption.final_sqlx_version,
        audit_id: adoption.audit_id,
    };
    let receipt_bytes = serde_json::to_vec_pretty(&receipt_payload).context(
        "legacy v28 adoption committed but encoding receipt failed; keep all roles stopped",
    )?;
    receipt
        .take()
        .ok_or_else(|| anyhow::anyhow!("reserved adoption receipt disappeared"))?
        .publish(&receipt_bytes)
        .context(
            "legacy v28 adoption committed but publishing receipt failed; keep all roles stopped",
        )?;

    println!(
        "postgres-adopt-legacy-v28 completed database={} legacy_version={} final_sqlx_version={} audit_id={} receipt={}",
        confirmed_database_name,
        receipt_payload.legacy_migration_version,
        receipt_payload.final_sqlx_version,
        receipt_payload.audit_id,
        receipt_path.display(),
    );
    Ok(())
}
