//! PostgreSQL rebuild 回执模型和不可覆盖发布流程。

use chrono::{DateTime, Utc};
use serde::Serialize;

use auction_storage::{PostgresRebuildPreflight, PostgresRebuildResult};

use super::model::{AuthorizationReceipt, Options, ReceiptContext};

use anyhow::Context;

use super::super::{PrivateReceiptReservation, RebuildReceiptSpec, sha256_text};

/// PostgreSQL rebuild 使用的不可覆盖回执预留器。
pub(super) type RebuildReceiptReservation = PrivateReceiptReservation<RebuildReceiptSpec>;
const POSTGRES_REBUILD_RECEIPT_FORMAT: &str = "auction-postgres-rebuild.v2";

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
    forced_drop: Option<bool>,
    database_dropped: Option<bool>,
    database_recreated: Option<bool>,
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
        template_database: (context.database_recreated == Some(true)).then_some("template0"),
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

/// 发布后置校验失败回执，保留已经证明的数据库变更事实。
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
            database_dropped: Some(true),
            database_recreated: Some(true),
            postcondition: None,
            migration_version: None,
        },
    )
    .context("database changed but publishing PostgreSQL rebuild failure receipt failed")
}
