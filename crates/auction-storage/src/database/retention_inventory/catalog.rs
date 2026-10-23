//! 外键、schema、迁移和 public 关系目录盘点。

use super::mapping::*;
use super::*;
use sqlx::{Row, postgres::PgRow};

pub(super) async fn read_foreign_key_inventory(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<PostgresForeignKeyInventoryReport, RetentionInventoryError> {
    let rows = sqlx::query(
        "SELECT
             source.relname AS source_table,
             target.relname AS referenced_table,
             constraint_row.conname AS constraint_name,
             CASE constraint_row.confdeltype
                 WHEN 'a' THEN 'no_action'
                 WHEN 'r' THEN 'restrict'
                 WHEN 'c' THEN 'cascade'
                 WHEN 'n' THEN 'set_null'
                 WHEN 'd' THEN 'set_default'
                 ELSE 'unknown'
             END AS delete_action,
             COUNT(*) OVER ()::bigint AS foreign_key_count
         FROM pg_constraint AS constraint_row
         JOIN pg_class AS source ON source.oid = constraint_row.conrelid
         JOIN pg_namespace AS source_namespace ON source_namespace.oid = source.relnamespace
         JOIN pg_class AS target ON target.oid = constraint_row.confrelid
         JOIN pg_namespace AS target_namespace ON target_namespace.oid = target.relnamespace
         WHERE constraint_row.contype = 'f'
           AND source_namespace.nspname = 'public'
           AND target_namespace.nspname = 'public'
         ORDER BY target.relname ASC, source.relname ASC, constraint_row.conname ASC
         LIMIT $1",
    )
    .bind(FOREIGN_KEY_QUERY_LIMIT)
    .fetch_all(&mut **transaction)
    .await
    .map_err(|source| query_error("foreign_key_inventory", source))?;
    let foreign_key_count = if let Some(row) = rows.first() {
        row.try_get("foreign_key_count")
            .map_err(|source| query_error("foreign_key_inventory", source))?
    } else {
        0
    };
    let relations = rows
        .into_iter()
        .take(MAX_FOREIGN_KEYS as usize)
        .map(map_foreign_key)
        .collect::<Result<Vec<_>, _>>()?;
    let reported_foreign_key_count =
        i64::try_from(relations.len()).map_err(|_| RetentionInventoryError::CountOverflow)?;
    Ok(PostgresForeignKeyInventoryReport {
        relations,
        coverage: PostgresForeignKeyCoverage {
            foreign_key_count,
            reported_foreign_key_count,
            foreign_key_rows_truncated: foreign_key_count > MAX_FOREIGN_KEYS,
            eligibility_assessed: false,
            eligibility_authorized: false,
        },
    })
}

pub(super) fn map_foreign_key(
    row: PgRow,
) -> Result<PostgresForeignKeyInventory, RetentionInventoryError> {
    Ok(PostgresForeignKeyInventory {
        source_table: row
            .try_get("source_table")
            .map_err(|source| query_error("foreign_key_inventory", source))?,
        referenced_table: row
            .try_get("referenced_table")
            .map_err(|source| query_error("foreign_key_inventory", source))?,
        constraint_name: row
            .try_get("constraint_name")
            .map_err(|source| query_error("foreign_key_inventory", source))?,
        delete_action: row
            .try_get("delete_action")
            .map_err(|source| query_error("foreign_key_inventory", source))?,
        dependency_is_eligibility_blocker_until_policy_defined: true,
        eligibility_assessed: false,
        eligibility_authorized: false,
    })
}

pub(super) async fn set_read_only_limits(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<(), RetentionInventoryError> {
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut **transaction)
        .await
        .map_err(|source| query_error("transaction_policy", source))?;
    sqlx::query("SET LOCAL statement_timeout = '5s'")
        .execute(&mut **transaction)
        .await
        .map_err(|source| query_error("statement_timeout", source))?;
    sqlx::query("SET LOCAL lock_timeout = '1s'")
        .execute(&mut **transaction)
        .await
        .map_err(|source| query_error("lock_timeout", source))?;
    Ok(())
}

pub(super) async fn read_schema_identity(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<PostgresSchemaIdentity, RetentionInventoryError> {
    let row = sqlx::query(
        "SELECT application, baseline_version, installed_at
         FROM public.auction_schema_identity
         WHERE singleton
         LIMIT 2",
    )
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|source| query_error("schema_identity", source))?;
    let Some(row) = row else {
        return Err(RetentionInventoryError::SchemaIdentityInvalid);
    };
    let application: String =
        row.try_get("application").map_err(|source| query_error("schema_identity", source))?;
    let baseline_version: i64 =
        row.try_get("baseline_version").map_err(|source| query_error("schema_identity", source))?;
    let installed_at: DateTime<Utc> =
        row.try_get("installed_at").map_err(|source| query_error("schema_identity", source))?;
    if application != "auction" || baseline_version <= 0 {
        return Err(RetentionInventoryError::SchemaIdentityInvalid);
    }
    Ok(PostgresSchemaIdentity { application, baseline_version, installed_at })
}

pub(super) async fn read_migration_inventory(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<PostgresMigrationInventory, RetentionInventoryError> {
    let rows = sqlx::query(
        "SELECT version, description, success, checksum
         FROM public._sqlx_migrations
         ORDER BY version",
    )
    .fetch_all(&mut **transaction)
    .await
    .map_err(|source| query_error("migration_identity", source))?;
    let migration_count =
        i64::try_from(rows.len()).map_err(|_| RetentionInventoryError::CountOverflow)?;
    let latest_version = rows
        .last()
        .map_or(Ok(0_i64), |row| row.try_get("version"))
        .map_err(|source| query_error("migration_summary", source))?;
    let latest_description = rows
        .last()
        .map(|row| row.try_get("description"))
        .transpose()
        .map_err(|source| query_error("migration_description", source))?;
    let mut all_success = true;
    for row in &rows {
        all_success &= row
            .try_get::<bool, _>("success")
            .map_err(|source| query_error("migration_summary", source))?;
    }
    let expected_version = crate::database::postgres_expected_migration_version();
    Ok(PostgresMigrationInventory {
        migration_count,
        latest_version,
        latest_description,
        all_success,
        expected_version,
        // current 只证明完整迁移身份；盘点结果不构成任何删除授权。
        current: super::super::migrations::postgres_history_matches(&rows, expected_version),
    })
}

pub(super) async fn read_relations(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<Vec<PostgresRelationInventory>, RetentionInventoryError> {
    let rows = sqlx::query(
        "SELECT namespace.nspname AS schema_name,
                relation.relname AS relation_name,
                relation.relkind::text AS relation_kind,
                COALESCE(stats.n_live_tup, 0)::bigint AS estimated_rows,
                COALESCE(stats.n_dead_tup, 0)::bigint AS estimated_dead_rows,
                relation.reltuples::bigint AS catalog_rows_estimate,
                stats.last_analyze AS last_analyze,
                stats.last_autoanalyze AS last_autoanalyze,
                pg_table_size(relation.oid)::bigint AS table_bytes,
                pg_indexes_size(relation.oid)::bigint AS index_bytes,
                pg_total_relation_size(relation.oid)::bigint AS total_bytes
         FROM pg_class AS relation
         JOIN pg_namespace AS namespace ON namespace.oid = relation.relnamespace
         LEFT JOIN pg_stat_user_tables AS stats ON stats.relid = relation.oid
         WHERE namespace.nspname = 'public'
           AND relation.relkind IN ('r', 'p', 'm', 'f')
         ORDER BY relation.relname ASC
         LIMIT $1",
    )
    .bind(RELATION_QUERY_LIMIT)
    .fetch_all(&mut **transaction)
    .await
    .map_err(|source| query_error("relation_inventory", source))?;
    let relation_count =
        i64::try_from(rows.len()).map_err(|_| RetentionInventoryError::CountOverflow)?;
    if relation_count > MAX_RELATIONS {
        return Err(RetentionInventoryError::TooManyRelations(MAX_RELATIONS));
    }

    rows.into_iter().map(map_relation).collect()
}

pub(super) fn map_relation(
    row: PgRow,
) -> Result<PostgresRelationInventory, RetentionInventoryError> {
    Ok(PostgresRelationInventory {
        schema_name: row
            .try_get("schema_name")
            .map_err(|source| query_error("relation_inventory", source))?,
        relation_name: row
            .try_get("relation_name")
            .map_err(|source| query_error("relation_inventory", source))?,
        relation_kind: row
            .try_get("relation_kind")
            .map_err(|source| query_error("relation_inventory", source))?,
        estimated_rows: row
            .try_get("estimated_rows")
            .map_err(|source| query_error("relation_inventory", source))?,
        estimated_dead_rows: row
            .try_get("estimated_dead_rows")
            .map_err(|source| query_error("relation_inventory", source))?,
        catalog_rows_estimate: row
            .try_get("catalog_rows_estimate")
            .map_err(|source| query_error("relation_inventory", source))?,
        last_analyze: row
            .try_get("last_analyze")
            .map_err(|source| query_error("relation_inventory", source))?,
        last_autoanalyze: row
            .try_get("last_autoanalyze")
            .map_err(|source| query_error("relation_inventory", source))?,
        table_bytes: row
            .try_get("table_bytes")
            .map_err(|source| query_error("relation_inventory", source))?,
        index_bytes: row
            .try_get("index_bytes")
            .map_err(|source| query_error("relation_inventory", source))?,
        total_bytes: row
            .try_get("total_bytes")
            .map_err(|source| query_error("relation_inventory", source))?,
    })
}
