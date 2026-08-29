use chrono::Utc;
use sqlx::{PgConnection, Row};

use super::model::{
    CollectedState, LocaleProvider, MAINTENANCE_DATABASE_NAME, PostgresRebuildError,
    PostgresRebuildPreflight, SUPPORTED_POSTGRES_MAJOR, TargetDatabaseDefinition,
};

pub(super) async fn configure_session(
    connection: &mut PgConnection,
) -> Result<(), PostgresRebuildError> {
    for (stage, statement) in [
        ("application_name", "SET application_name = 'auctionctl/postgres-rebuild'"),
        ("search_path", "SET search_path = pg_catalog"),
        ("standard_conforming_strings", "SET standard_conforming_strings = on"),
        ("statement_timeout", "SET statement_timeout = '30s'"),
        ("lock_timeout", "SET lock_timeout = '5s'"),
    ] {
        sqlx::query(statement)
            .execute(&mut *connection)
            .await
            .map_err(|source| query_error(stage, source))?;
    }
    Ok(())
}

pub(super) async fn collect_state(
    connection: &mut PgConnection,
    target_database_name: &str,
    advisory_lock_acquired: bool,
) -> Result<CollectedState, PostgresRebuildError> {
    let cluster = sqlx::query(
        "SELECT current_database() AS maintenance_database_name,
                current_user AS current_user,
                current_setting('server_version_num')::integer AS server_version_num,
                pg_is_in_recovery() AS in_recovery,
                current_setting('default_transaction_read_only') = 'on'
                    AS default_transaction_read_only,
                role.rolsuper AS current_user_superuser,
                role.rolcreatedb AS current_user_createdb
         FROM pg_catalog.pg_roles AS role
         WHERE role.rolname = current_user",
    )
    .fetch_one(&mut *connection)
    .await
    .map_err(|source| query_error("cluster_identity", source))?;
    let target = sqlx::query(
        "SELECT database.oid::bigint AS database_oid,
                pg_catalog.pg_get_userbyid(database.datdba) AS database_owner,
                pg_catalog.pg_encoding_to_char(database.encoding) AS encoding,
                database.datlocprovider::text AS locale_provider,
                database.datcollate AS lc_collate,
                database.datctype AS lc_ctype,
                database.datlocale AS provider_locale,
                database.daticurules AS icu_rules,
                tablespace.spcname AS tablespace,
                database.datconnlimit AS connection_limit,
                database.datallowconn AS allows_connections,
                database.datistemplate AS is_template,
                database.datacl IS NOT NULL AS has_custom_acl,
                EXISTS (
                    SELECT 1 FROM pg_catalog.pg_db_role_setting AS setting
                    WHERE setting.setdatabase = database.oid
                ) AS has_role_settings,
                (
                    tablespace.spcname = 'pg_default'
                    OR has_tablespace_privilege(current_user, database.dattablespace, 'CREATE')
                ) AS can_create_in_tablespace,
                (SELECT COUNT(*)::bigint FROM pg_catalog.pg_stat_activity
                 WHERE datname = database.datname) AS active_sessions,
                (SELECT COUNT(*)::bigint FROM pg_catalog.pg_prepared_xacts AS prepared
                 WHERE prepared.database = database.datname) AS prepared_transactions,
                (SELECT COUNT(*)::bigint FROM pg_catalog.pg_replication_slots AS slot
                 WHERE slot.database = database.datname AND slot.active) AS active_logical_slots,
                (SELECT COUNT(*)::bigint FROM pg_catalog.pg_subscription AS subscription
                 WHERE subscription.subdbid = database.oid) AS subscriptions
         FROM pg_catalog.pg_database AS database
         JOIN pg_catalog.pg_tablespace AS tablespace
           ON tablespace.oid = database.dattablespace
         WHERE database.datname = $1",
    )
    .bind(target_database_name)
    .fetch_optional(&mut *connection)
    .await
    .map_err(|source| query_error("target_database", source))?
    .ok_or(PostgresRebuildError::UnsafePreflight {
        reason: "confirmed target database does not exist",
    })?;

    let maintenance_database_name: String = row_value(&cluster, "maintenance_database_name")?;
    let current_user: String = row_value(&cluster, "current_user")?;
    let server_version_num: i32 = row_value(&cluster, "server_version_num")?;
    let in_recovery: bool = row_value(&cluster, "in_recovery")?;
    let default_transaction_read_only: bool = row_value(&cluster, "default_transaction_read_only")?;
    let current_user_superuser: bool = row_value(&cluster, "current_user_superuser")?;
    let current_user_createdb: bool = row_value(&cluster, "current_user_createdb")?;
    let target_database_oid: i64 = row_value(&target, "database_oid")?;
    let target_owner: String = row_value(&target, "database_owner")?;
    let encoding: String = row_value(&target, "encoding")?;
    let locale_provider_code: String = row_value(&target, "locale_provider")?;
    let locale_provider = LocaleProvider::from_catalog(&locale_provider_code)?;
    let lc_collate: String = row_value(&target, "lc_collate")?;
    let lc_ctype: String = row_value(&target, "lc_ctype")?;
    let provider_locale: Option<String> = row_value(&target, "provider_locale")?;
    let icu_rules: Option<String> = row_value(&target, "icu_rules")?;
    let tablespace: String = row_value(&target, "tablespace")?;
    let connection_limit: i32 = row_value(&target, "connection_limit")?;
    let target_allows_connections: bool = row_value(&target, "allows_connections")?;
    let target_is_template: bool = row_value(&target, "is_template")?;
    let target_has_custom_acl: bool = row_value(&target, "has_custom_acl")?;
    let target_has_role_settings: bool = row_value(&target, "has_role_settings")?;
    let current_user_can_create_in_tablespace: bool =
        row_value(&target, "can_create_in_tablespace")?;
    let active_target_sessions: i64 = row_value(&target, "active_sessions")?;
    let prepared_transactions: i64 = row_value(&target, "prepared_transactions")?;
    let active_logical_replication_slots: i64 = row_value(&target, "active_logical_slots")?;
    let subscriptions: i64 = row_value(&target, "subscriptions")?;
    let current_user_owns_target = current_user == target_owner;
    let rebuild_permitted = maintenance_database_name == MAINTENANCE_DATABASE_NAME
        && server_version_num / 10_000 == SUPPORTED_POSTGRES_MAJOR
        && !in_recovery
        && !default_transaction_read_only
        && target_allows_connections
        && !target_is_template
        && !target_has_custom_acl
        && !target_has_role_settings
        && encoding == "UTF8"
        && connection_limit >= -1
        && (current_user_superuser || (current_user_owns_target && current_user_createdb))
        && (current_user_superuser || current_user_can_create_in_tablespace)
        && active_target_sessions == 0
        && prepared_transactions == 0
        && active_logical_replication_slots == 0
        && subscriptions == 0
        && advisory_lock_acquired;

    Ok(CollectedState {
        preflight: PostgresRebuildPreflight {
            format: super::model::POSTGRES_REBUILD_PREFLIGHT_FORMAT,
            collected_at: Utc::now(),
            maintenance_database_name,
            server_version_num,
            target_database_name: target_database_name.to_owned(),
            target_database_oid,
            target_owner,
            target_encoding: encoding.clone(),
            target_locale_provider: locale_provider.name().to_owned(),
            target_tablespace: tablespace.clone(),
            target_connection_limit: connection_limit,
            target_allows_connections,
            target_is_template,
            target_has_custom_acl,
            target_has_role_settings,
            current_user,
            current_user_superuser,
            current_user_createdb,
            current_user_owns_target,
            current_user_can_create_in_tablespace,
            in_recovery,
            default_transaction_read_only,
            active_target_sessions,
            prepared_transactions,
            active_logical_replication_slots,
            subscriptions,
            advisory_lock_acquired,
            rebuild_permitted,
        },
        definition: TargetDatabaseDefinition {
            encoding,
            locale_provider,
            lc_collate,
            lc_ctype,
            provider_locale,
            icu_rules,
            tablespace,
            connection_limit,
        },
    })
}

pub(super) fn require_safe_preflight(
    preflight: &PostgresRebuildPreflight,
) -> Result<(), PostgresRebuildError> {
    let reason = if preflight.maintenance_database_name != MAINTENANCE_DATABASE_NAME {
        Some("maintenance connection is not attached to the postgres database")
    } else if preflight.server_version_num / 10_000 != SUPPORTED_POSTGRES_MAJOR {
        Some("server major version is outside the supported PostgreSQL 18 contract")
    } else if preflight.in_recovery || preflight.default_transaction_read_only {
        Some("maintenance connection is read-only or attached to a recovery replica")
    } else if !preflight.target_allows_connections || preflight.target_is_template {
        Some("target database is connection-disabled or marked as a template")
    } else if preflight.target_has_custom_acl {
        Some("target database has a custom ACL that this rebuild contract will not discard")
    } else if preflight.target_has_role_settings {
        Some(
            "target database has database/role settings that this rebuild contract will not discard",
        )
    } else if preflight.target_encoding != "UTF8" {
        Some("target database encoding is not UTF8")
    } else if preflight.target_connection_limit < -1 {
        Some("target database connection limit is invalid")
    } else if !(preflight.current_user_superuser
        || (preflight.current_user_owns_target && preflight.current_user_createdb))
    {
        Some("maintenance role must be superuser or the target owner with CREATEDB")
    } else if !(preflight.current_user_superuser || preflight.current_user_can_create_in_tablespace)
    {
        Some("maintenance role cannot recreate the database in its current tablespace")
    } else if preflight.active_target_sessions != 0 {
        Some("target database still has active sessions; stop every role and client first")
    } else if preflight.prepared_transactions != 0 {
        Some("target database has prepared transactions")
    } else if preflight.active_logical_replication_slots != 0 {
        Some("target database has active logical replication slots")
    } else if preflight.subscriptions != 0 {
        Some("target database has subscriptions")
    } else if !preflight.advisory_lock_acquired {
        Some("database rebuild advisory lock is not held")
    } else if !preflight.rebuild_permitted {
        Some("database rebuild preflight did not reach a permitted state")
    } else {
        None
    };
    match reason {
        Some(reason) => Err(PostgresRebuildError::UnsafePreflight { reason }),
        None => Ok(()),
    }
}

pub(super) fn query_error(stage: &'static str, source: sqlx::Error) -> PostgresRebuildError {
    PostgresRebuildError::Query { stage, source }
}

fn row_value<T>(
    row: &sqlx::postgres::PgRow,
    column: &'static str,
) -> Result<T, PostgresRebuildError>
where
    for<'decode> T: sqlx::Decode<'decode, sqlx::Postgres> + sqlx::Type<sqlx::Postgres>,
{
    row.try_get(column).map_err(|source| query_error("decode_catalog_row", source))
}
