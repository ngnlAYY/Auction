//! 000134 运行账号矩阵、函数 ACL 收口与重授权幂等。

use std::collections::BTreeSet;

use auction_storage::postgres_migrate_to;
use sqlx::PgPool;

use super::support::{fails, succeeds};
use crate::support::TestDatabase;

const PROJECTIONS: &[(&str, &[&str])] = &[
    (
        "consignment_mail_readiness",
        &[
            "session_id",
            "service_epoch",
            "policy_digest",
            "allowlist_mode",
            "allowed_senders",
            "validator_versions",
            "detail_ready",
            "take_ready",
            "return_ready",
            "checked_at",
            "valid_until",
            "version",
        ],
    ),
    (
        "mail_evidence_envelopes",
        &["ciphertext", "nonce", "encryption_key_id", "fingerprint", "updated_at", "retired_at"],
    ),
    ("mail_quarantines", &["version", "state", "updated_at"]),
    (
        "mail_quarantine_rechecks",
        &[
            "state",
            "available_at",
            "lease_token",
            "lease_until",
            "fence",
            "attempt_count",
            "completed_at",
        ],
    ),
    ("consignment_deposit_reservations", &["state", "last_command_id", "retired_at", "reason"]),
    (
        "consignment_reservation_secrets",
        &["ciphertext", "nonce", "encryption_key_id", "fingerprint", "updated_at", "retired_at"],
    ),
    (
        "consignment_equipment_claims",
        &["revision", "reservation_id", "position_id", "changed_at", "last_fact_id"],
    ),
    ("consignment_custody_positions", &["state", "version", "current_generation", "updated_at"]),
];

const TABLES: [&str; 29] = [
    "consignment_season_windows",
    "mail_evidence_sessions",
    "consignment_mail_readiness",
    "mail_originals",
    "mail_original_links",
    "mail_evidence_envelopes",
    "mail_envelope_observations",
    "mail_observation_conflicts",
    "mail_initial_decisions",
    "mail_quarantines",
    "mail_quarantine_rechecks",
    "mail_final_dispositions",
    "mail_quarantine_releases",
    "settlement_recovery_return_handoffs",
    "consignment_deposit_reservations",
    "consignment_reservation_secrets",
    "consignment_equipment_claims",
    "consignment_custody_assets",
    "consignment_deposit_items",
    "consignment_custody_positions",
    "consignment_disposition_generations",
    "consignment_custody_events",
    "consignment_custody_controls",
    "consignment_equipment_handoffs",
    "consignment_season_notices",
    "mail_disposition_work",
    "mail_disposition_attempts",
    "mail_disposition_write_starts",
    "mail_disposition_proofs",
];

async fn assert_matrix(pool: &PgPool) {
    for table in TABLES {
        let privileges: (bool, bool, bool, bool, bool, bool, bool) = sqlx::query_as(
            "SELECT has_table_privilege($1, 'SELECT'), has_table_privilege($1, 'INSERT'),
                    has_table_privilege($1, 'UPDATE'), has_table_privilege($1, 'DELETE'),
                    has_table_privilege($1, 'TRUNCATE'), has_table_privilege($1, 'REFERENCES'),
                    has_table_privilege($1, 'TRIGGER')",
        )
        .bind(format!("public.{table}"))
        .fetch_one(pool)
        .await
        .expect("读取新表权限");
        assert_eq!(privileges, (true, true, false, false, false, false, false), "{table}");
    }
    for (table, expected) in PROJECTIONS {
        let mut actual: Vec<String> = sqlx::query_scalar(
            "SELECT attname::text FROM pg_attribute
             WHERE attrelid=$1::regclass AND attnum>0 AND NOT attisdropped
               AND has_column_privilege(attrelid, attname, 'UPDATE')",
        )
        .bind(format!("public.{table}"))
        .fetch_all(pool)
        .await
        .expect("读取投影列权限");
        let mut expected = expected.to_vec();
        actual.sort_unstable();
        expected.sort_unstable();
        assert_eq!(actual, expected, "{table}");
    }
    for (table, _) in PROJECTIONS {
        let all_columns: Vec<String> = sqlx::query_scalar(
            "SELECT attname::text FROM pg_attribute
             WHERE attrelid=$1::regclass AND attnum>0 AND NOT attisdropped",
        )
        .bind(format!("public.{table}"))
        .fetch_all(pool)
        .await
        .expect("读取投影全部列");
        for column in all_columns {
            if !PROJECTIONS
                .iter()
                .find(|(name, _)| name == table)
                .is_some_and(|(_, columns)| columns.contains(&column.as_str()))
            {
                fails(
                    pool,
                    &format!("UPDATE public.{table} SET {column}={column} WHERE false"),
                    "42501",
                    None,
                )
                .await;
            }
        }
    }
}

#[tokio::test]
async fn postgres_consignment_deposit_runtime_acl_matches_the_declared_matrix() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = runtime.postgres_pool();
    assert_matrix(pool).await;
    let function_acl: (bool, bool, bool) = sqlx::query_as(
        "SELECT has_function_privilege('auction_runtime',
                    'public.apply_auction_runtime_privileges()', 'EXECUTE'),
                has_function_privilege('auction_runtime',
                    'public.apply_auction_runtime_privileges_000132()', 'EXECUTE'),
                has_function_privilege('auction_runtime',
                    'public.apply_auction_runtime_privileges()', 'EXECUTE')",
    )
    .fetch_one(pool)
    .await
    .expect("读取函数执行权");
    assert_eq!(function_acl, (false, false, false));
    let public_execute: i64 = sqlx::query_scalar(
        "SELECT count(*)::bigint
         FROM pg_proc AS procedure
         CROSS JOIN LATERAL aclexplode(COALESCE(
             procedure.proacl, acldefault('f'::\"char\", procedure.proowner))) AS acl
         WHERE procedure.oid IN (
             'public.apply_auction_runtime_privileges()'::regprocedure,
             'public.apply_auction_runtime_privileges_000132()'::regprocedure)
           AND acl.grantee = 0 AND acl.privilege_type = 'EXECUTE'",
    )
    .fetch_one(pool)
    .await
    .expect("读取 PUBLIC 函数执行权");
    assert_eq!(public_execute, 0);

    succeeds(
        owner.postgres_pool(),
        "REVOKE ALL ON ALL TABLES IN SCHEMA public FROM auction_runtime;
         SELECT public.apply_auction_runtime_privileges();",
    )
    .await;
    assert_matrix(pool).await;

    succeeds(
        owner.postgres_pool(),
        "GRANT EXECUTE ON FUNCTION public.apply_auction_runtime_privileges() TO PUBLIC;
         GRANT EXECUTE ON FUNCTION public.apply_auction_runtime_privileges() TO auction_runtime;
         GRANT EXECUTE ON FUNCTION public.apply_auction_runtime_privileges_000132() TO PUBLIC;
         GRANT EXECUTE ON FUNCTION public.apply_auction_runtime_privileges_000132() TO auction_runtime;
         SELECT public.apply_auction_runtime_privileges();",
    )
    .await;
    let restored: (bool, bool, i64) = sqlx::query_as(
        "SELECT has_function_privilege('auction_runtime',
                    'public.apply_auction_runtime_privileges()', 'EXECUTE'),
                has_function_privilege('auction_runtime',
                    'public.apply_auction_runtime_privileges_000132()', 'EXECUTE'),
                (SELECT count(*)::bigint FROM pg_proc AS procedure
                 CROSS JOIN LATERAL aclexplode(COALESCE(
                     procedure.proacl, acldefault('f'::\"char\", procedure.proowner))) AS acl
                 WHERE procedure.oid IN (
                     'public.apply_auction_runtime_privileges()'::regprocedure,
                     'public.apply_auction_runtime_privileges_000132()'::regprocedure)
                   AND acl.grantee = 0 AND acl.privilege_type = 'EXECUTE')",
    )
    .fetch_one(pool)
    .await
    .expect("读取恢复后的函数 ACL");
    assert_eq!(restored, (false, false, 0));
}

#[tokio::test]
async fn postgres_consignment_deposit_runtime_acl_rejects_public_write_grants() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    fails(
        owner.postgres_pool(),
        "GRANT UPDATE ON public.consignment_season_windows TO PUBLIC;
         SELECT public.apply_auction_runtime_privileges();",
        "42501",
        None,
    )
    .await;
}

async fn non_consignment_acls(pool: &PgPool) -> BTreeSet<String> {
    let rows: Vec<(String, String, String, String, String)> = sqlx::query_as(
        "WITH relations AS (
             SELECT relation.oid, relation.relname, relation.relkind, relation.relacl
             FROM pg_class AS relation
             JOIN pg_namespace AS namespace ON namespace.oid = relation.relnamespace
             WHERE namespace.nspname='public'
               AND relation.relkind IN ('r','p','v','m','f','S')
               AND NOT starts_with(relation.relname::text, 'consignment_')
         ),
         relation_acl AS (
             SELECT 'relation'::text AS kind, relname::text AS object_name,
                    relkind::text AS detail, acl.grantee,
                    acl.privilege_type::text AS privilege_type
             FROM relations CROSS JOIN LATERAL aclexplode(COALESCE(
                 relacl, acldefault(CASE WHEN relkind='S' THEN 's'::\"char\" ELSE 'r'::\"char\" END,
                                    (SELECT relowner FROM pg_class WHERE oid=relations.oid)))) AS acl
         ),
         column_acl AS (
             SELECT 'column'::text AS kind, relation.relname::text AS object_name,
                    attribute.attname::text AS detail, acl.grantee,
                    acl.privilege_type::text AS privilege_type
             FROM relations AS relation
             JOIN pg_attribute AS attribute ON attribute.attrelid=relation.oid
               AND attribute.attnum>0 AND NOT attribute.attisdropped
             CROSS JOIN LATERAL aclexplode(attribute.attacl) AS acl
         ),
         function_acl AS (
             SELECT 'function'::text AS kind, procedure.proname::text AS object_name,
                    pg_get_function_identity_arguments(procedure.oid)::text AS detail,
                    acl.grantee, acl.privilege_type::text AS privilege_type
             FROM pg_proc AS procedure
             JOIN pg_namespace AS namespace ON namespace.oid=procedure.pronamespace
             CROSS JOIN LATERAL aclexplode(COALESCE(
                 procedure.proacl, acldefault('f'::\"char\", procedure.proowner))) AS acl
             WHERE namespace.nspname='public'
               AND NOT starts_with(procedure.proname::text, 'consignment_')
               AND NOT starts_with(procedure.proname::text, 'c4_')
               AND procedure.proname <> 'apply_auction_runtime_privileges_000132'
         ),
         all_acl AS (
             SELECT * FROM relation_acl UNION ALL
             SELECT * FROM column_acl UNION ALL
             SELECT * FROM function_acl
         )
         SELECT kind, object_name, detail,
                CASE WHEN grantee=0 THEN 'PUBLIC'
                     ELSE COALESCE(role.rolname::text, grantee::text) END,
                privilege_type
         FROM all_acl LEFT JOIN pg_roles AS role ON role.oid=all_acl.grantee",
    )
    .fetch_all(pool)
    .await
    .expect("读取非寄售 ACL");
    rows.into_iter()
        .map(|(kind, name, detail, grantee, privilege)| {
            format!("{kind}|{name}|{detail}|{grantee}|{privilege}")
        })
        .collect()
}

#[tokio::test]
async fn postgres_consignment_deposit_acl_keeps_existing_objects_unchanged_and_is_idempotent() {
    let database = TestDatabase::create_empty().await.expect("创建空库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    postgres_migrate_to(pool, 133).await.expect("迁移到 133");
    let baseline = non_consignment_acls(pool).await;
    assert!(!baseline.is_empty());
    postgres_migrate_to(pool, 134).await.expect("迁移到 134");
    for rebuild in [false, true] {
        if rebuild {
            succeeds(
                owner.postgres_pool(),
                "REVOKE ALL ON ALL TABLES IN SCHEMA public FROM auction_runtime;
                 SELECT public.apply_auction_runtime_privileges();",
            )
            .await;
        }
        let actual = non_consignment_acls(pool).await;
        let removed: Vec<_> = baseline.difference(&actual).collect();
        assert!(removed.is_empty(), "既有对象授权被撤销或改写：{removed:?}");
    }
}

async fn runtime_matrix(pool: &PgPool) -> BTreeSet<String> {
    let mut matrix = BTreeSet::new();
    for table in TABLES {
        let (select, insert, update, delete, truncate, references, trigger): (
            bool,
            bool,
            bool,
            bool,
            bool,
            bool,
            bool,
        ) = sqlx::query_as(
            "SELECT has_table_privilege('auction_runtime', $1, 'SELECT'),
                    has_table_privilege('auction_runtime', $1, 'INSERT'),
                    has_table_privilege('auction_runtime', $1, 'UPDATE'),
                    has_table_privilege('auction_runtime', $1, 'DELETE'),
                    has_table_privilege('auction_runtime', $1, 'TRUNCATE'),
                    has_table_privilege('auction_runtime', $1, 'REFERENCES'),
                    has_table_privilege('auction_runtime', $1, 'TRIGGER')",
        )
        .bind(format!("public.{table}"))
        .fetch_one(pool)
        .await
        .expect("读取表权限");
        matrix.insert(format!(
            "table|{table}|{select}|{insert}|{update}|{delete}|{truncate}|{references}|{trigger}"
        ));
        let mut columns: Vec<String> = sqlx::query_scalar(
            "SELECT attname::text FROM pg_attribute
             WHERE attrelid=$1::regclass AND attnum>0 AND NOT attisdropped
               AND has_column_privilege(attrelid, attname, 'UPDATE')
             ORDER BY attname",
        )
        .bind(format!("public.{table}"))
        .fetch_all(pool)
        .await
        .expect("读取列级更新权限");
        columns.sort_unstable();
        matrix.insert(format!("columns|{table}|{}", columns.join(",")));
    }
    let sequences: Vec<(String, bool, bool, bool)> = sqlx::query_as(
        "SELECT relation.relname::text,
                has_sequence_privilege('auction_runtime', relation.oid, 'USAGE'),
                has_sequence_privilege('auction_runtime', relation.oid, 'SELECT'),
                has_sequence_privilege('auction_runtime', relation.oid, 'UPDATE')
         FROM pg_class AS relation
         JOIN pg_namespace AS namespace ON namespace.oid = relation.relnamespace
         WHERE namespace.nspname='public' AND relation.relkind='S'
         ORDER BY relation.relname",
    )
    .fetch_all(pool)
    .await
    .expect("读取序列权限");
    for (name, usage, select, update) in sequences {
        matrix.insert(format!("sequence|{name}|{usage}|{select}|{update}"));
    }
    let functions: Vec<(String, bool)> = sqlx::query_as(
        "SELECT procedure.oid::regprocedure::text,
                has_function_privilege('auction_runtime', procedure.oid, 'EXECUTE')
         FROM pg_proc AS procedure
         JOIN pg_namespace AS namespace ON namespace.oid = procedure.pronamespace
         WHERE namespace.nspname='public'
           AND (procedure.proname LIKE 'c4\\_%' ESCAPE '\\'
                OR procedure.proname LIKE 'consignment\\_%' ESCAPE '\\'
                OR procedure.proname LIKE 'apply\\_auction\\_runtime\\_privileges%' ESCAPE '\\')
         ORDER BY procedure.oid",
    )
    .fetch_all(pool)
    .await
    .expect("读取函数权限");
    for (name, execute) in functions {
        matrix.insert(format!("function|{name}|{execute}"));
    }
    matrix
}

#[tokio::test]
async fn postgres_deposit_runtime_acl_survives_grants_replay() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    let before = runtime_matrix(pool).await;
    assert!(!before.is_empty());

    succeeds(
        owner.postgres_pool(),
        "REVOKE ALL ON ALL TABLES IN SCHEMA public FROM auction_runtime;
         SELECT public.apply_auction_runtime_privileges();",
    )
    .await;
    let after = runtime_matrix(pool).await;
    assert_eq!(before, after, "重放授权必须逐项恢复原矩阵");

    let ddl: (bool, bool, bool) = sqlx::query_as(
        "SELECT has_schema_privilege('auction_runtime', 'public', 'CREATE'),
                has_database_privilege('auction_runtime', current_database(), 'CREATE'),
                has_schema_privilege('auction_runtime', 'public', 'USAGE')",
    )
    .fetch_one(pool)
    .await
    .expect("读取 DDL 权限");
    assert_eq!(ddl, (false, false, true), "运行账号不得获得建对象权限");

    let projected: BTreeSet<&str> = PROJECTIONS.iter().map(|(table, _)| *table).collect();
    for table in TABLES {
        let (update_any, delete, truncate): (bool, bool, bool) = sqlx::query_as(
            "SELECT has_any_column_privilege('auction_runtime', $1, 'UPDATE'),
                    has_table_privilege('auction_runtime', $1, 'DELETE'),
                    has_table_privilege('auction_runtime', $1, 'TRUNCATE')",
        )
        .bind(format!("public.{table}"))
        .fetch_one(pool)
        .await
        .expect("读取宽写权限");
        assert!(!delete, "{table} 不得有 DELETE");
        assert!(!truncate, "{table} 不得有 TRUNCATE");
        if !projected.contains(table) {
            assert!(!update_any, "只追加表 {table} 不得有列级 UPDATE");
        }
    }
}
