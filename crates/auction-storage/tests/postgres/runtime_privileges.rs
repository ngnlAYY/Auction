//! 验收：服务进程以运行账号连接时不能执行 DDL、`TRUNCATE`，也不能改写只追加表。
//!
//! 运行账号权限由迁移 081/097 的 `apply_auction_runtime_privileges` 授予：全部表只读，写入面逐表
//! 显式授权，只追加表只授予 `INSERT`。表 owner 天然拥有 DDL 与 `TRUNCATE`，无法靠 REVOKE 收口，
//! 因此服务必须以非 owner 的运行账号连接。这里断言权限层（SQLSTATE `42501`）先于只追加触发器拒绝。
//! 入口见 `mise run test-db`。

#![allow(clippy::expect_used, clippy::unwrap_used)]

use auction_storage::{DatabaseStore, apply_runtime_privileges};

use crate::support;

/// 迁移以触发器保护的只追加表（抽样覆盖竞价、审计、结算、资金、投递证明、开关事实、通知模板修订
/// 与结算退件证据）。
const APPEND_ONLY_TABLES: [&str; 11] = [
    "audit_records",
    "bids",
    "auction_settlements",
    "lot_settlements",
    "compensation_ledger",
    "delivery_dispositions",
    "delivery_sent_confirmations",
    "runtime_switch_events",
    "mail_template_events",
    "settlement_return_evidence",
    "user_blacklist_history",
];

const INSUFFICIENT_PRIVILEGE: &str = "42501";

/// `000122` 改名得到的链尾入口与 `000131` 新建的公开入口；三者都必须只允许 owner 执行。
const RUNTIME_PRIVILEGE_FUNCTIONS: [&str; 3] = [
    "public.apply_auction_runtime_privileges()",
    "public.apply_auction_runtime_privileges_000117()",
    "public.apply_auction_runtime_privileges_000122()",
];

/// 执行一条语句并返回数据库错误码；成功返回 `None`。
async fn sqlstate(store: &DatabaseStore, sql: &str) -> Option<String> {
    match sqlx::query(sqlx::AssertSqlSafe(sql.to_owned())).execute(store.postgres_pool()).await {
        Ok(_) => None,
        Err(error) => {
            Some(error.as_database_error().and_then(sqlx::error::DatabaseError::code).map_or_else(
                || format!("non-database error: {error}"),
                std::borrow::Cow::into_owned,
            ))
        }
    }
}

async fn first_column(store: &DatabaseStore, table: &str) -> String {
    sqlx::query_scalar(
        "SELECT column_name::text FROM information_schema.columns
         WHERE table_schema = 'public' AND table_name = $1
         ORDER BY ordinal_position LIMIT 1",
    )
    .bind(table)
    .fetch_one(store.postgres_pool())
    .await
    .expect("读取表的首列")
}

/// 指定角色是否持有函数的 EXECUTE 权限。
async fn function_executable(store: &DatabaseStore, role: &str, function: &str) -> bool {
    sqlx::query_scalar(
        "SELECT pg_catalog.has_function_privilege($1, $2::pg_catalog.regprocedure, 'EXECUTE')",
    )
    .bind(role)
    .bind(function)
    .fetch_one(store.postgres_pool())
    .await
    .expect("读取角色函数执行授权")
}

/// PUBLIC 是否直接持有函数的 EXECUTE 权限；默认 ACL 也按 `acldefault` 展开后判断。
async fn public_executable(store: &DatabaseStore, function: &str) -> bool {
    sqlx::query_scalar(
        "SELECT EXISTS (
             SELECT 1
             FROM pg_catalog.pg_proc AS procedure
             CROSS JOIN LATERAL pg_catalog.aclexplode(COALESCE(
                 procedure.proacl,
                 pg_catalog.acldefault('f'::\"char\", procedure.proowner)
             )) AS acl
             WHERE procedure.oid = $1::pg_catalog.regprocedure
               AND acl.grantee = 0
               AND acl.privilege_type = 'EXECUTE'
         )",
    )
    .bind(function)
    .fetch_one(store.postgres_pool())
    .await
    .expect("读取 PUBLIC 函数执行授权")
}

/// 把三个授权链函数的 ACL 置回 `--no-privileges` 恢复后的默认状态，并显式授给运行账号。
async fn restore_default_function_acls(store: &DatabaseStore) {
    for function in RUNTIME_PRIVILEGE_FUNCTIONS {
        let statement = format!("GRANT EXECUTE ON FUNCTION {function} TO PUBLIC, auction_runtime");
        sqlx::query(sqlx::AssertSqlSafe(statement))
            .execute(store.postgres_pool())
            .await
            .expect("恢复默认函数 ACL");
    }
}

#[tokio::test]
async fn postgres_runtime_account_cannot_run_ddl_truncate_or_rewrite_append_only_tables() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let maintenance = database.maintenance_store().await.expect("以维护账号连接");

    // 前提：被测连接确实是 auction_runtime 的非超级用户成员，而不是 owner。
    let (superuser, member, owns_tables): (bool, bool, bool) = sqlx::query_as(
        "SELECT r.rolsuper,
                pg_has_role(current_user, 'auction_runtime', 'MEMBER'),
                EXISTS (SELECT 1 FROM pg_tables
                        WHERE schemaname = 'public' AND tableowner = current_user)
         FROM pg_roles r WHERE r.rolname = current_user",
    )
    .fetch_one(runtime.postgres_pool())
    .await
    .expect("读取当前角色");
    assert!(!superuser && member && !owns_tables, "被测连接必须是非 owner 的运行账号成员");

    // DDL：建表、改表、建索引、删表都在权限层被拒绝。
    for sql in [
        "CREATE TABLE public.w99_runtime_probe (id integer)",
        "ALTER TABLE public.audit_records ADD COLUMN w99_runtime_probe integer",
        "CREATE INDEX w99_runtime_probe ON public.bids (lot_id)",
        "DROP TABLE public.audit_records",
    ] {
        assert_eq!(sqlstate(&runtime, sql).await.as_deref(), Some(INSUFFICIENT_PRIVILEGE), "{sql}");
    }

    for table in APPEND_ONLY_TABLES {
        let column = first_column(&runtime, table).await;
        for sql in [
            format!("TRUNCATE public.{table}"),
            format!("UPDATE public.{table} SET {column} = {column} WHERE false"),
            format!("DELETE FROM public.{table} WHERE false"),
        ] {
            assert_eq!(
                sqlstate(&runtime, &sql).await.as_deref(),
                Some(INSUFFICIENT_PRIVILEGE),
                "{sql}"
            );
        }
    }

    // 对照：同一库上维护账号可以执行 DDL，说明拒绝来自运行账号的权限而不是语句本身。
    assert_eq!(
        sqlstate(&maintenance, "CREATE TABLE public.w99_runtime_probe (id integer)").await,
        None
    );
    assert_eq!(sqlstate(&maintenance, "DROP TABLE public.w99_runtime_probe").await, None);

    runtime.postgres_pool().close().await;
    maintenance.postgres_pool().close().await;
}

#[tokio::test]
async fn postgres_runtime_privileges_replay_closes_entire_function_chain() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let maintenance = database.maintenance_store().await.expect("以维护账号连接");

    // 模拟 `--no-privileges` 恢复：三个函数回到 PUBLIC 可执行，运行账号也能直接执行。
    restore_default_function_acls(&maintenance).await;
    for function in RUNTIME_PRIVILEGE_FUNCTIONS {
        assert!(public_executable(&maintenance, function).await, "{function} 应处于默认 ACL");
        assert!(
            function_executable(&maintenance, "auction_runtime", function).await,
            "恢复后 auction_runtime 应能执行 {function}"
        );
    }

    apply_runtime_privileges(maintenance.postgres_pool()).await.expect("重放运行账号授权函数");

    for function in RUNTIME_PRIVILEGE_FUNCTIONS {
        assert!(!public_executable(&maintenance, function).await, "PUBLIC 不得执行 {function}");
        assert!(
            !function_executable(&maintenance, "auction_runtime", function).await,
            "auction_runtime 不得执行 {function}"
        );
        let statement = format!("SELECT {function}");
        assert_eq!(
            sqlstate(&runtime, &statement).await.as_deref(),
            Some(INSUFFICIENT_PRIVILEGE),
            "{statement}"
        );
    }

    runtime.postgres_pool().close().await;
    maintenance.postgres_pool().close().await;
}

#[tokio::test]
async fn postgres_runtime_privileges_replay_keeps_funding_source_write_limits() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let maintenance = database.maintenance_store().await.expect("以维护账号连接");

    restore_default_function_acls(&maintenance).await;
    apply_runtime_privileges(maintenance.postgres_pool()).await.expect("重放运行账号授权函数");

    let table_update: bool = sqlx::query_scalar(
        "SELECT pg_catalog.has_table_privilege(
             'auction_runtime', 'public.settlement_funding_sources', 'UPDATE'
         )",
    )
    .fetch_one(maintenance.postgres_pool())
    .await
    .expect("读取来源表 UPDATE 授权");
    assert!(!table_update, "运行账号不得拥有 settlement_funding_sources 整表 UPDATE");

    let issued_amount_update: bool = sqlx::query_scalar(
        "SELECT pg_catalog.has_column_privilege(
             'auction_runtime', 'public.settlement_funding_sources', 'issued_amount', 'UPDATE'
         )",
    )
    .fetch_one(maintenance.postgres_pool())
    .await
    .expect("读取 issued_amount 列 UPDATE 授权");
    assert!(!issued_amount_update, "运行账号不得改写 issued_amount");

    assert_eq!(
        sqlstate(
            &runtime,
            "UPDATE public.settlement_funding_sources SET issued_amount = issued_amount WHERE false"
        )
        .await
        .as_deref(),
        Some(INSUFFICIENT_PRIVILEGE)
    );

    runtime.postgres_pool().close().await;
    maintenance.postgres_pool().close().await;
}
