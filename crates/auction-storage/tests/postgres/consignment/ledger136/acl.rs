//! 000136 授权：事实表对运行账号只有 SELECT、INSERT，可变投影只有指定列的 UPDATE；restore 后重放授权链仍收口。

use auction_storage::apply_runtime_privileges;
use sqlx::PgPool;

use super::super::deposit::support::{fails, succeeds};
use crate::support::TestDatabase;

const FACT_TABLES: [&str; 18] = [
    "consignment_handoff_proofs",
    "consignment_dispatch_rounds",
    "consignment_dispatch_attempts",
    "consignment_dispatch_evidence",
    "consignment_return_intakes",
    "consignment_return_conflicts",
    "consignment_return_consumptions",
    "consignment_return_facts",
    "consignment_return_fact_verdicts",
    "consignment_refusals",
    "consignment_sanction_members",
    "consignment_work_closures",
    "consignment_cod_receipts",
    "consignment_opening_slices",
    "consignment_opening_slice_uses",
    "consignment_fee_facts",
    "consignment_principal_facts",
    "consignment_funding_returns",
];
const PROJECTIONS: [(&str, &[&str]); 5] = [
    ("consignment_refusal_cycles", &["close_reason", "closed_at", "state"]),
    ("consignment_sanctions", &["state", "version"]),
    ("consignment_account_work", &["state", "version"]),
    ("consignment_cash_return_blocks", &["cleared_at", "cleared_by", "state"]),
    ("consignment_funded_sales", &["disposition", "version"]),
];
const CHAIN: [&str; 6] = [
    "public.apply_auction_runtime_privileges()",
    "public.apply_auction_runtime_privileges_000135()",
    "public.apply_auction_runtime_privileges_000134()",
    "public.apply_auction_runtime_privileges_000132()",
    "public.apply_auction_runtime_privileges_000122()",
    "public.apply_auction_runtime_privileges_000117()",
];

async fn first_column(pool: &PgPool, table: &str) -> String {
    sqlx::query_scalar(
        "SELECT column_name::text FROM information_schema.columns
         WHERE table_schema = 'public' AND table_name = $1 ORDER BY ordinal_position LIMIT 1",
    )
    .bind(table)
    .fetch_one(pool)
    .await
    .expect("读取首列")
}

async fn table_flags(pool: &PgPool, table: &str) -> (bool, bool, bool, bool, bool, bool, bool) {
    sqlx::query_as(
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
    .expect("读取表权限")
}

async fn updatable_columns(pool: &PgPool, table: &str) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT attname::text FROM pg_attribute
         WHERE attrelid = $1::regclass AND attnum > 0 AND NOT attisdropped
           AND has_column_privilege('auction_runtime', attrelid, attname, 'UPDATE')
         ORDER BY attname",
    )
    .bind(format!("public.{table}"))
    .fetch_all(pool)
    .await
    .expect("读取列级更新权限")
}

async fn assert_matrix(pool: &PgPool) {
    for table in FACT_TABLES.iter() {
        assert_eq!(
            table_flags(pool, table).await,
            (true, true, false, false, false, false, false),
            "{table}"
        );
        assert!(updatable_columns(pool, table).await.is_empty(), "{table} 不得有列级 UPDATE");
    }
    for (table, columns) in PROJECTIONS {
        assert_eq!(
            table_flags(pool, table).await,
            (true, true, false, false, false, false, false),
            "{table}"
        );
        assert_eq!(updatable_columns(pool, table).await, columns, "{table} 只有状态列可更新");
    }
}

async fn function_closed(pool: &PgPool, function: &str) -> bool {
    let (public, runtime): (bool, bool) = sqlx::query_as(
        "SELECT EXISTS (
                    SELECT 1 FROM pg_proc AS procedure
                    CROSS JOIN LATERAL aclexplode(COALESCE(
                        procedure.proacl, acldefault('f'::\"char\", procedure.proowner))) AS acl
                    WHERE procedure.oid = $1::regprocedure
                      AND acl.grantee = 0 AND acl.privilege_type = 'EXECUTE'),
                has_function_privilege('auction_runtime', $1::regprocedure, 'EXECUTE')",
    )
    .bind(function)
    .fetch_one(pool)
    .await
    .expect("读取函数执行权");
    !public && !runtime
}

#[tokio::test]
async fn postgres_consignment_ledger136_runtime_acl_is_append_only_plus_projection_columns() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    assert_matrix(pool).await;
    // 反例：运行账号对事实表的改写在权限层（42501）被拒；投影表的非状态列同样被拒。
    for table in FACT_TABLES.iter() {
        let column = first_column(pool, table).await;
        for sql in [
            format!("UPDATE public.{table} SET {column} = {column} WHERE false"),
            format!("DELETE FROM public.{table} WHERE false"),
            format!("TRUNCATE public.{table}"),
        ] {
            fails(pool, &sql, "42501", None).await;
        }
    }
    fails(pool, "UPDATE public.consignment_sanctions SET threshold = 3 WHERE false", "42501", None)
        .await;
    fails(pool, "DELETE FROM public.consignment_cash_return_blocks WHERE false", "42501", None)
        .await;
}

#[tokio::test]
async fn postgres_consignment_ledger136_acl_replays_after_restore() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let runtime = database.store().await.expect("以运行账号连接");

    // 模拟 pg_restore --no-privileges：授权链各函数回到 PUBLIC 可执行，运行账号的表授权消失。
    let mut restore =
        String::from("REVOKE ALL ON ALL TABLES IN SCHEMA public FROM auction_runtime;");
    for function in CHAIN {
        restore
            .push_str(&format!("GRANT EXECUTE ON FUNCTION {function} TO PUBLIC, auction_runtime;"));
    }
    succeeds(owner.postgres_pool(), &restore).await;
    for function in CHAIN {
        assert!(
            !function_closed(owner.postgres_pool(), function).await,
            "{function} 应处于默认 ACL"
        );
    }

    apply_runtime_privileges(owner.postgres_pool()).await.expect("重放运行账号授权");

    for function in CHAIN {
        assert!(function_closed(owner.postgres_pool(), function).await, "{function} 必须收口");
    }
    assert_matrix(runtime.postgres_pool()).await;
    // 重放幂等。
    succeeds(owner.postgres_pool(), "SELECT public.apply_auction_runtime_privileges();").await;
    assert_matrix(runtime.postgres_pool()).await;
}
