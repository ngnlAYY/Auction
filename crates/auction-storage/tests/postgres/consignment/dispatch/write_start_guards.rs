//! 起写的锁序与凭据绑定：按合同顺序加锁、锁验 active 凭据并冻结进 Send 身份。
//!
//! 保护不变量 2、4：起写与按合同顺序（挂单 → position → 履约行 → 凭据 → 投递）加锁的事务不成环；
//! 凭据已非 active 或版本不符时拒绝，不发许可、不留任何起写事实。

use std::time::Duration;

use auction_application::consignment::{
    ConsignmentError, DepositBusinessRejection, DispatchStore, MarkWriteStarted, WriteStart,
    WriteStartOutcome,
};
use auction_application::core::RequestHash;
use auction_application::delivery::DeliverySendCredentialBinding;
use auction_domain::CommandId;
use auction_domain::consignment::dispatch::AttemptKey;
use sqlx::PgPool;
use uuid::Uuid;

use super::super::deposit::support::{CREDENTIAL, POSITION};
use super::fixture::{PRICE, Seeded, frozen, round_key, uuid};

fn binding(version: i64) -> DeliverySendCredentialBinding {
    DeliverySendCredentialBinding {
        credential_id: uuid(CREDENTIAL),
        credential_version: version,
        runtime_generation: 1,
    }
}

/// 领取并登记尝试，返回尝试键与 claim token。
async fn claimed_attempt(seeded: &Seeded) -> (AttemptKey, Uuid) {
    let token = Uuid::new_v4();
    seeded.claim(token, 1).await;
    let attempt = AttemptKey { delivery_id: seeded.delivery_id, fence: 1 };
    seeded
        .service()
        .start_attempt(round_key(), attempt, token, frozen(PRICE))
        .await
        .expect("登记尝试");
    (attempt, token)
}

/// 夹具凭据的 scope 是简写；起写锁验认的是 Worker 侧的 `mooglemail:<world>` 命名空间。
/// 持凭据绑定的起写先锁运行时 generation，夹具同时提交绑定所指的 generation 1。
async fn use_worker_scope(seeded: &Seeded, scope: &str) {
    sqlx::query("UPDATE public.credential_keys SET scope = $2 WHERE id = $1")
        .bind(uuid(CREDENTIAL))
        .bind(scope)
        .execute(seeded.pool())
        .await
        .expect("设置凭据 scope");
    sqlx::query(
        "INSERT INTO public.mail_runtime_generation (scope, generation, activated_at)
         VALUES ('mail_cookie', 1, clock_timestamp()) ON CONFLICT (scope) DO NOTHING",
    )
    .execute(seeded.pool())
    .await
    .expect("提交运行时 generation");
}

async fn started_facts(seeded: &Seeded) -> (String, i64) {
    sqlx::query_as(
        "SELECT (SELECT status FROM public.deliveries WHERE id = $1),
                (SELECT count(*) FROM public.delivery_send_attempts WHERE delivery_id = $1)",
    )
    .bind(seeded.delivery_id)
    .fetch_one(seeded.pool())
    .await
    .expect("投递与冻结尝试")
}

#[tokio::test]
async fn postgres_consignment_dispatch_write_start_freezes_the_active_credential() {
    let seeded = Seeded::purchased().await;
    use_worker_scope(&seeded, "mooglemail:main").await;
    let (attempt, token) = claimed_attempt(&seeded).await;
    let started = seeded
        .service()
        .mark_write_started(attempt, token, Some(binding(1)))
        .await
        .expect("active 凭据通过");
    assert!(matches!(started, WriteStart::FirstCommitted { .. }));
    // 冻结的 Send 身份写的是实际绑定，之后可把发送关联回凭据版本。
    let frozen_credential: serde_json::Value = sqlx::query_scalar(
        "SELECT expectation -> 'credential' FROM public.delivery_send_attempts
         WHERE delivery_id = $1",
    )
    .bind(seeded.delivery_id)
    .fetch_one(seeded.pool())
    .await
    .expect("冻结凭据");
    assert_eq!(
        frozen_credential,
        serde_json::json!({
            "credential_id": CREDENTIAL,
            "credential_version": 1,
            "runtime_generation": 1,
        })
    );
}

#[tokio::test]
async fn postgres_consignment_dispatch_write_start_rejects_a_stale_credential_without_a_permit() {
    // 版本不符、凭据已被停用、凭据属于另一个世界、运行时已换代：都确定拒绝且不留起写事实。
    for case in ["version", "disabled", "other_world", "generation"] {
        let seeded = Seeded::purchased().await;
        use_worker_scope(
            &seeded,
            if case == "other_world" { "mooglemail:isekai" } else { "mooglemail:main" },
        )
        .await;
        let (attempt, token) = claimed_attempt(&seeded).await;
        if case == "disabled" {
            sqlx::query("UPDATE public.credential_keys SET status = 'disabled' WHERE id = $1")
                .bind(uuid(CREDENTIAL))
                .execute(seeded.pool())
                .await
                .expect("停用凭据");
        }
        if case == "generation" {
            // 新 owner 推进 generation 后，旧绑定的起写不得拿到许可。
            sqlx::query("UPDATE public.mail_runtime_generation SET generation = 2")
                .execute(seeded.pool())
                .await
                .expect("推进 generation");
        }
        let version = if case == "version" { 2 } else { 1 };
        let denied = seeded
            .service()
            .mark_write_started(attempt, token, Some(binding(version)))
            .await
            .expect_err("凭据失效必须拒绝");
        assert_eq!(
            denied,
            ConsignmentError::Deposit(DepositBusinessRejection::ActionNotAllowed),
            "{case}"
        );
        assert_eq!(started_facts(&seeded).await, ("claimed".to_owned(), 0), "{case}");
    }
}

/// 等到至少一个会话正在等行锁。
async fn wait_for_lock_waiter(pool: &PgPool) {
    for _ in 0..150 {
        let waiting: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_stat_activity
             WHERE datname = current_database() AND wait_event_type = 'Lock'
               AND pid <> pg_backend_pid()",
        )
        .fetch_one(pool)
        .await
        .expect("读取等待会话");
        if waiting > 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("起写没有在 position 行锁上等待");
}

#[tokio::test]
async fn postgres_consignment_dispatch_write_start_waits_on_position_without_holding_the_delivery()
{
    let seeded = Seeded::purchased().await;
    let token = Uuid::new_v4();
    seeded.claim(token, 1).await;
    let attempt = AttemptKey { delivery_id: seeded.delivery_id, fence: 1 };
    // 持有者按合同顺序先取 position，再取投递（购买、收款与 position 迁移的次序）。
    let mut holder = seeded.pool().begin().await.expect("开启持有事务");
    sqlx::query(
        "SELECT 1 FROM public.consignment_custody_positions WHERE id = $1::uuid FOR UPDATE",
    )
    .bind(POSITION)
    .execute(&mut *holder)
    .await
    .expect("持有 position");
    let runtime = seeded.runtime.clone();
    let start = tokio::spawn(async move {
        runtime
            .mark_write_started(MarkWriteStarted {
                command_id: CommandId::from_uuid(Uuid::from_u128(
                    0x5200_0000_0000_5000_8000_0000_0000_0001,
                )),
                request_hash: RequestHash::new([9; 32]),
                attempt,
                claim_token: token,
                credential: Some(binding(1)),
            })
            .await
    });
    wait_for_lock_waiter(seeded.pool()).await;
    // 起写正等 position：它此刻不得已持有投递行锁，否则这一步与它互等成环（只有死锁检测能解开）。
    sqlx::query("SELECT 1 FROM public.deliveries WHERE id = $1 FOR UPDATE NOWAIT")
        .bind(seeded.delivery_id)
        .execute(&mut *holder)
        .await
        .expect("起写等 position 时不持有投递行锁");
    holder.commit().await.expect("释放持有事务");
    // 许可唯一：持有者放手后起写完成，恰一条冻结尝试与一条寄售尝试。
    let outcome = start.await.expect("起写任务").expect("起写完成");
    assert!(matches!(outcome, WriteStartOutcome::FirstCommitted(_)), "{outcome:?}");
    let attempts: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM public.delivery_send_attempts WHERE delivery_id = $1),
                (SELECT count(*) FROM public.consignment_dispatch_attempts WHERE delivery_id = $1)",
    )
    .bind(seeded.delivery_id)
    .fetch_one(seeded.pool())
    .await
    .expect("尝试行数");
    assert_eq!(attempts, (1, 1));
}
