//! 000135 授权：三张合同表对运行账号只有 SELECT、INSERT；restore 后重放授权链仍收口。

use auction_storage::apply_runtime_privileges;
use sqlx::PgPool;

use super::super::deposit::support::{fails, succeeds};
use crate::support::TestDatabase;

const TABLES: [&str; 3] = ["consignment_contracts", "consignment_sales", "consignment_withdrawals"];
const CHAIN: [&str; 4] = [
    "public.apply_auction_runtime_privileges()",
    "public.apply_auction_runtime_privileges_000135()",
    "public.apply_auction_runtime_privileges_000134()",
    "public.apply_auction_runtime_privileges_000132()",
];

async fn assert_select_insert_only(pool: &PgPool) {
    for table in TABLES {
        let privileges: (bool, bool, bool, bool, bool, bool, bool) = sqlx::query_as(
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
        .expect("读取合同表权限");
        assert_eq!(privileges, (true, true, false, false, false, false, false), "{table}");
        let column_updates: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_attribute
             WHERE attrelid = $1::regclass AND attnum > 0 AND NOT attisdropped
               AND has_column_privilege('auction_runtime', attrelid, attname, 'UPDATE')",
        )
        .bind(format!("public.{table}"))
        .fetch_one(pool)
        .await
        .expect("读取合同表列级更新权限");
        assert_eq!(column_updates, 0, "{table} 不得有列级 UPDATE");
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
async fn postgres_consignment_contract_tables_runtime_acl_is_select_insert_only() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    assert_select_insert_only(pool).await;
    // 反例：运行账号的写入在权限层（42501）被拒，先于只追加触发器。
    for table in TABLES {
        for sql in [
            format!("UPDATE public.{table} SET kind = kind WHERE false"),
            format!("DELETE FROM public.{table} WHERE false"),
            format!("TRUNCATE public.{table}"),
        ] {
            fails(pool, &sql, "42501", None).await;
        }
    }
}

#[tokio::test]
async fn postgres_consignment_contract_acl_replays_after_restore() {
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
    assert_select_insert_only(runtime.postgres_pool()).await;
    // 重放幂等：再执行一次结果不变。
    succeeds(owner.postgres_pool(), "SELECT public.apply_auction_runtime_privileges();").await;
    assert_select_insert_only(runtime.postgres_pool()).await;
}
