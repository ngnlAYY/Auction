//! 验收：服务进程以运行账号连接时不能执行 DDL、`TRUNCATE`，也不能改写只追加表。
//!
//! 运行账号权限由迁移 081/097 的 `apply_auction_runtime_privileges` 授予：全部表只读，写入面逐表
//! 显式授权，只追加表只授予 `INSERT`。表 owner 天然拥有 DDL 与 `TRUNCATE`，无法靠 REVOKE 收口，
//! 因此服务必须以非 owner 的运行账号连接。这里断言权限层（SQLSTATE `42501`）先于只追加触发器拒绝。
//! 入口见 `just test-db`。

#![cfg(feature = "test-db")]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use crate::support;

use auction_storage::DatabaseStore;

/// 迁移以触发器保护的只追加表（抽样覆盖竞价、审计、结算、资金、投递证明、开关事实与通知模板修订）。
const APPEND_ONLY_TABLES: [&str; 10] = [
    "audit_records",
    "bids",
    "auction_settlements",
    "lot_settlements",
    "compensation_ledger",
    "delivery_dispositions",
    "delivery_sent_confirmations",
    "runtime_switch_events",
    "mail_template_events",
    "user_blacklist_history",
];

const INSUFFICIENT_PRIVILEGE: &str = "42501";

/// 执行一条语句并返回数据库错误码；成功返回 `None`。
async fn sqlstate(store: &DatabaseStore, sql: &str) -> Option<String> {
    match sqlx::query(sqlx::AssertSqlSafe(sql.to_owned())).execute(store.postgres_pool()).await {
        Ok(_) => None,
        Err(error) => Some(
            error
                .as_database_error()
                .and_then(|database| database.code())
                .map(|code| code.into_owned())
                .unwrap_or_else(|| format!("non-database error: {error}")),
        ),
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
