//! PostgreSQL rebuild 回执模型和不可覆盖发布流程。

use chrono::{DateTime, Utc};
use serde::Serialize;

use auction_storage::{PostgresRebuildPreflight, PostgresRebuildResult};

use super::model::{AuthorizationReceipt, Options, ReceiptContext};

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::Context;

use super::super::{
    create_private_temp_path, require_absent_artifact, sha256_text, validate_private_evidence_path,
};

/// 在 mutation 前预留私有临时文件，成功或失败后只能发布一次目标回执。
pub(super) struct RebuildReceiptReservation {
    target_path: PathBuf,
    temporary_path: PathBuf,
    published: bool,
}

impl RebuildReceiptReservation {
    pub(super) fn reserve(target_path: &Path) -> anyhow::Result<Self> {
        validate_private_evidence_path(target_path)?;
        require_absent_artifact(target_path, "rebuild receipt")?;
        let parent = target_path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("rebuild receipt parent is missing"))?;
        let metadata = fs::symlink_metadata(parent)
            .with_context(|| format!("read rebuild receipt parent: {}", parent.display()))?;
        if !metadata.file_type().is_dir() {
            anyhow::bail!("rebuild receipt parent is not a directory: {}", parent.display());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o022 != 0 {
                anyhow::bail!(
                    "rebuild receipt parent is group/other writable: {}",
                    parent.display()
                );
            }
        }
        let temporary_path =
            create_private_temp_path(target_path, "auction-postgres-rebuild-receipt")?;
        Ok(Self { target_path: target_path.to_owned(), temporary_path, published: false })
    }

    pub(super) fn publish(mut self, contents: &[u8]) -> anyhow::Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&self.temporary_path)
            .with_context(|| {
                format!("open rebuild receipt temporary file: {}", self.temporary_path.display())
            })?;
        file.write_all(contents).context("write PostgreSQL rebuild receipt")?;
        file.write_all(b"\n").context("terminate PostgreSQL rebuild receipt")?;
        file.sync_all().context("sync PostgreSQL rebuild receipt")?;
        drop(file);

        require_absent_artifact(&self.target_path, "rebuild receipt")?;
        fs::hard_link(&self.temporary_path, &self.target_path).with_context(|| {
            format!("publish PostgreSQL rebuild receipt: {}", self.target_path.display())
        })?;
        fs::remove_file(&self.temporary_path).with_context(|| {
            format!(
                "rebuild receipt published but temporary cleanup failed: {}",
                self.temporary_path.display()
            )
        })?;
        self.published = true;
        Ok(())
    }
}

impl Drop for RebuildReceiptReservation {
    fn drop(&mut self) {
        if !self.published {
            let _ = fs::remove_file(&self.temporary_path);
        }
    }
}
const POSTGRES_REBUILD_RECEIPT_FORMAT: &str = "auction-postgres-rebuild.v1";

#[derive(Debug, Serialize)]
struct PostgresRebuildReceipt<'a> {
    format: &'static str,
    recorded_at: DateTime<Utc>,
    status: &'static str,
    failure_stage: Option<&'static str>,
    database_name: &'a str,
    database_name_sha256: String,
    mode: &'static str,
    authorization: &'a AuthorizationReceipt,
    source_schema_version: i64,
    previous_database_oid: i64,
    new_database_oid: Option<i64>,
    database_owner: &'a str,
    locale_provider: &'a str,
    tablespace: &'a str,
    connection_limit: i32,
    template_database: Option<&'static str>,
    forced_drop: bool,
    database_dropped: bool,
    database_recreated: bool,
    postcondition: Option<&'static str>,
    migration_version: Option<i64>,
}

pub(super) fn publish_receipt(
    reservation: &mut Option<RebuildReceiptReservation>,
    context: ReceiptContext<'_>,
) -> anyhow::Result<()> {
    let rebuild = context.rebuild;
    let payload = PostgresRebuildReceipt {
        format: POSTGRES_REBUILD_RECEIPT_FORMAT,
        recorded_at: Utc::now(),
        status: context.status,
        failure_stage: context.failure_stage,
        database_name: context.options.confirmed_database_name,
        database_name_sha256: sha256_text(context.options.confirmed_database_name),
        mode: context.options.mode.name(),
        authorization: context.authorization,
        source_schema_version: context.source_schema_version,
        previous_database_oid: context.preflight.target_database_oid,
        new_database_oid: rebuild.map(|result| result.new_database_oid),
        database_owner: &context.preflight.target_owner,
        locale_provider: rebuild
            .map_or(context.preflight.target_locale_provider.as_str(), |result| {
                result.locale_provider.as_str()
            }),
        tablespace: rebuild.map_or(context.preflight.target_tablespace.as_str(), |result| {
            result.tablespace.as_str()
        }),
        connection_limit: rebuild
            .map_or(context.preflight.target_connection_limit, |result| result.connection_limit),
        template_database: context.database_recreated.then_some("template0"),
        forced_drop: context.database_dropped,
        database_dropped: context.database_dropped,
        database_recreated: context.database_recreated,
        postcondition: context.postcondition,
        migration_version: context.migration_version,
    };
    let bytes = serde_json::to_vec_pretty(&payload).context("encode PostgreSQL rebuild receipt")?;
    reservation
        .take()
        .ok_or_else(|| {
            anyhow::anyhow!("required PostgreSQL rebuild receipt reservation is missing")
        })?
        .publish(&bytes)
}

pub(super) fn publish_postcondition_failure(
    reservation: &mut Option<RebuildReceiptReservation>,
    options: &Options<'_>,
    authorization: &AuthorizationReceipt,
    source_schema_version: i64,
    preflight: &PostgresRebuildPreflight,
    rebuild: &PostgresRebuildResult,
    failure_stage: &'static str,
) -> anyhow::Result<()> {
    publish_receipt(
        reservation,
        ReceiptContext {
            options,
            authorization,
            source_schema_version,
            preflight,
            rebuild: Some(rebuild),
            status: "postcondition_failed",
            failure_stage: Some(failure_stage),
            database_dropped: true,
            database_recreated: true,
            postcondition: None,
            migration_version: None,
        },
    )
    .context("database changed but publishing PostgreSQL rebuild failure receipt failed")
}
