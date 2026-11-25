//! 000137 授权：事实表对运行账号只有 SELECT、INSERT，outbox 只有租约列与（供换钥的）密文三列可更新，订单复核工作
//! 只有租约列与领取快照可更新；历史事实连维护账号也改不了；
//! restore 后重放授权链仍收口（不变量 7、8）。

use auction_storage::apply_runtime_privileges;
use sqlx::PgPool;

use super::super::deposit::support::{fails, succeeds};
use super::{PROOF, T0, WORK, command_ids, loss_chain_sql, seed_notice};
use crate::support::TestDatabase;

const FACT_TABLES: [&str; 20] = [
    "consignment_loss_proofs",
    "consignment_loss_facts",
    "consignment_loss_cases",
    "consignment_loss_case_outcomes",
    "consignment_loss_receipts",
    "consignment_loss_reconciles",
    "consignment_rights",
    "consignment_right_states",
    "consignment_right_challenges",
    "consignment_right_challenge_attempts",
    "consignment_right_sessions",
    "consignment_right_session_tokens",
    "consignment_right_notice_results",
    "consignment_payment_reminders",
    "consignment_relist_decisions",
    "consignment_right_request_hits",
    "consignment_sanction_effects",
    "consignment_sanction_work_results",
    "consignment_order_resolutions",
    "consignment_order_recheck_results",
];
const OUTBOX: &str = "consignment_right_notice_outbox";
/// 订单复核工作（⑳）：只有租约列与领取快照可更新。
const RECHECK_WORK: &str = "consignment_order_rechecks";
const CHAIN: [&str; 7] = [
    "public.apply_auction_runtime_privileges()",
    "public.apply_auction_runtime_privileges_000136()",
    "public.apply_auction_runtime_privileges_000135()",
    "public.apply_auction_runtime_privileges_000134()",
    "public.apply_auction_runtime_privileges_000132()",
    "public.apply_auction_runtime_privileges_000122()",
    "public.apply_auction_runtime_privileges_000117()",
];

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
    for table in FACT_TABLES.iter().chain([&OUTBOX, &RECHECK_WORK]) {
        assert_eq!(
            table_flags(pool, table).await,
            (true, true, false, false, false, false, false),
            "{table}"
        );
    }
    for table in FACT_TABLES {
        assert!(updatable_columns(pool, table).await.is_empty(), "{table} 不得有列级 UPDATE");
    }
    assert_eq!(
        updatable_columns(pool, OUTBOX).await,
        [
            "claim_fence",
            "claim_token",
            "encryption_key_id",
            "lease_expires_at",
            "nonce",
            "sealed_payload"
        ],
        "outbox 只有租约列与密文三列可更新"
    );
    assert_eq!(
        updatable_columns(pool, RECHECK_WORK).await,
        [
            "claim_fence",
            "claim_token",
            "lease_expires_at",
            "pending_dispatch_attempts",
            "pending_take_works"
        ],
        "订单复核工作只有租约列与领取快照可更新"
    );
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
async fn postgres_consignment_recovery_tables_runtime_acl_is_select_insert_only() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let runtime = database.store().await.expect("以运行账号连接");
    let pool = runtime.postgres_pool();
    assert_matrix(pool).await;
    // 反例：运行账号的改写在权限层（42501）被拒；outbox 的非租约列同样不可改。
    for table in FACT_TABLES.iter().chain([&OUTBOX, &RECHECK_WORK]) {
        for sql in
            [format!("DELETE FROM public.{table} WHERE false"), format!("TRUNCATE public.{table}")]
        {
            fails(pool, &sql, "42501", None).await;
        }
    }
    for table in FACT_TABLES {
        let first: String = sqlx::query_scalar(
            "SELECT column_name::text FROM information_schema.columns
             WHERE table_schema = 'public' AND table_name = $1 ORDER BY ordinal_position LIMIT 1",
        )
        .bind(table)
        .fetch_one(pool)
        .await
        .expect("读取首列");
        fails(
            pool,
            &format!("UPDATE public.{table} SET {first} = {first} WHERE false"),
            "42501",
            None,
        )
        .await;
    }
    fails(
        pool,
        &format!("UPDATE public.{OUTBOX} SET frozen_identity = frozen_identity WHERE false"),
        "42501",
        None,
    )
    .await;
}

#[tokio::test]
async fn postgres_consignment_recovery_facts_reject_mutation_even_for_the_owner() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    seed_notice(pool).await;
    let commands = command_ids(pool, 3).await;
    succeeds(pool, &loss_chain_sql(&commands)).await;

    // 反例：维护账号也改不了、删不了已写下的事实；追加式不依赖权限层。
    fails(
        pool,
        &format!("UPDATE public.consignment_loss_proofs SET recorded_at = '{T0}' WHERE proof_id = '{PROOF}'"),
        "55000",
        None,
    )
    .await;
    fails(
        pool,
        &format!("DELETE FROM public.consignment_loss_proofs WHERE proof_id = '{PROOF}'"),
        "55000",
        None,
    )
    .await;
    fails(pool, &format!("DELETE FROM public.{OUTBOX} WHERE work_id = '{WORK}'"), "55000", None)
        .await;
    // 结构：全部事实表都挂了拒 UPDATE/DELETE 与拒 TRUNCATE 两个触发器。
    let missing: Vec<String> = sqlx::query_scalar(
        "SELECT table_name FROM unnest($1::text[]) AS table_name
         WHERE (SELECT count(*) FROM pg_trigger AS trigger
                WHERE trigger.tgrelid = ('public.' || table_name)::regclass
                  AND trigger.tgfoid = 'public.consignment_reject_fact_mutation'::regproc) <> 2",
    )
    // 订单复核工作（⑳）的 UPDATE 由租约守卫 c7_order_recheck_guard 接管，拒 DELETE 与拒 TRUNCATE 仍是这两个触发器。
    .bind(
        FACT_TABLES
            .iter()
            .chain([&RECHECK_WORK])
            .map(|table| (*table).to_owned())
            .collect::<Vec<_>>(),
    )
    .fetch_all(pool)
    .await
    .expect("读取触发器");
    assert!(missing.is_empty(), "缺少追加触发器：{missing:?}");
}

#[tokio::test]
async fn postgres_consignment_recovery_runtime_can_only_move_the_outbox_lease() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    seed_notice(owner.postgres_pool()).await;
    let runtime = database.store().await.expect("以运行账号连接");
    // 正例：运行账号可领取（租约列）。
    succeeds(
        runtime.postgres_pool(),
        &format!(
            "UPDATE public.{OUTBOX}
             SET claim_token = '7c600000-0000-4000-8000-0000000000aa', claim_fence = 1,
                 lease_expires_at = '2026-01-03T00:10:00Z'
             WHERE work_id = '{WORK}'"
        ),
    )
    .await;
    // 反例：密文列只对整体换钥开放（受控分支见 rights/key_rotation.rs），任意覆写被守卫拒绝。
    fails(
        runtime.postgres_pool(),
        &format!("UPDATE public.{OUTBOX} SET sealed_payload = decode('00', 'hex') WHERE work_id = '{WORK}'"),
        "23514",
        Some("c7_notice_immutable"),
    )
    .await;
}

#[tokio::test]
async fn postgres_consignment_recovery_acl_replays_after_restore() {
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
    // 重放幂等：再执行一次结果不变。
    succeeds(owner.postgres_pool(), "SELECT public.apply_auction_runtime_privileges();").await;
    assert_matrix(runtime.postgres_pool()).await;
}
