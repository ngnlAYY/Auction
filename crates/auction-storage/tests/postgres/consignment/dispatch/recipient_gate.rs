//! 寄售轮最终 Send 授权点：改名受益人零外写、黑名单不剥夺既有订单权利、起写与订单阶段事件同事务、
//! 与结算锁序并发不死锁。

use std::time::Duration;

use auction_application::consignment::{ConsignmentError, DepositBusinessRejection, WriteStart};
use auction_domain::consignment::dispatch::AttemptKey;
use sqlx::PgPool;
use uuid::Uuid;

use super::super::contracts::fixture::{CONTRACT, OBLIGATION_1};
use super::super::deposit::support::USER_2;
use super::fixture::{PRICE, Seeded, binding, frozen, round_key};

/// 外写面：冻结 Send 尝试与寄售尝试行数。
async fn send_writes(pool: &PgPool, delivery: Uuid) -> (i64, i64) {
    sqlx::query_as(
        "SELECT (SELECT count(*) FROM public.delivery_send_attempts WHERE delivery_id = $1),
                (SELECT count(*) FROM public.consignment_dispatch_attempts WHERE delivery_id = $1)",
    )
    .bind(delivery)
    .fetch_one(pool)
    .await
    .expect("读取外写面")
}

/// 该合同的订单阶段事件数。
async fn order_events(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM public.realtime_events WHERE stream_key = $1")
        .bind(format!("consignment_order:{CONTRACT}"))
        .fetch_one(pool)
        .await
        .expect("订单阶段事件数")
}

async fn delivery_status(pool: &PgPool, delivery: Uuid) -> String {
    sqlx::query_scalar("SELECT status FROM public.deliveries WHERE id = $1")
        .bind(delivery)
        .fetch_one(pool)
        .await
        .expect("投递状态")
}

/// 购买、领取并登记尝试，返回尝试键与领取 token。
async fn attempt_ready(seeded: &Seeded) -> (AttemptKey, Uuid) {
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

/// 维护账号改受益用户（改名或拉黑）；SQL 是固定文本。
async fn set_user(pool: &PgPool, sql: &'static str) {
    sqlx::query(sql).bind(USER_2).execute(pool).await.expect("改受益用户");
}

const RENAME: &str = "UPDATE public.users SET username = 'FakeRenamedBuyer' WHERE id = $1::uuid";
const BLACKLIST: &str = "UPDATE public.users SET status = 'blacklisted' WHERE id = $1::uuid";

#[tokio::test]
async fn postgres_consignment_dispatch_write_start_retains_a_renamed_beneficiary_with_zero_send_writes()
 {
    let seeded = Seeded::purchased().await;
    let pool = seeded.pool();
    let (attempt, token) = attempt_ready(&seeded).await;
    let events_before = order_events(pool).await;
    // 改名后冻结名可能已属他人：身份连续性未证，保留权利、不发许可。
    set_user(pool, RENAME).await;

    let error = seeded
        .service()
        .mark_write_started(attempt, token, Some(binding()))
        .await
        .expect_err("改名受益人不得起写");
    assert_eq!(error, ConsignmentError::Deposit(DepositBusinessRejection::EvidenceUnproven));
    assert_eq!(send_writes(pool, seeded.delivery_id).await, (0, 0));
    assert_eq!(delivery_status(pool, seeded.delivery_id).await, "claimed");
    assert_eq!(order_events(pool).await, events_before);
}

#[tokio::test]
async fn postgres_consignment_dispatch_write_start_still_serves_a_blacklisted_beneficiary_with_one_event()
 {
    let seeded = Seeded::purchased().await;
    let pool = seeded.pool();
    let (attempt, token) = attempt_ready(&seeded).await;
    let events_before = order_events(pool).await;
    // 黑名单只阻断新购，不剥夺已成交订单的履约权利。
    set_user(pool, BLACKLIST).await;

    let started =
        seeded.service().mark_write_started(attempt, token, Some(binding())).await.expect("起写");
    assert!(matches!(started, WriteStart::FirstCommitted { .. }), "{started:?}");
    assert_eq!(send_writes(pool, seeded.delivery_id).await, (1, 1));
    assert_eq!(order_events(pool).await, events_before + 1);
}

#[tokio::test]
async fn postgres_consignment_dispatch_write_start_rolls_back_with_its_order_event() {
    let seeded = Seeded::purchased().await;
    let pool = seeded.pool();
    let (attempt, token) = attempt_ready(&seeded).await;
    let events_before = order_events(pool).await;
    // 故障：订单阶段事件写入失败。起写、尝试与 position 推进必须随之整笔回滚。
    sqlx::raw_sql(
        "CREATE FUNCTION public.test_reject_order_event() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN
             IF NEW.kind = 'consignment.order_stage_changed' THEN
                 RAISE EXCEPTION 'test order event rejected';
             END IF;
             RETURN NEW;
         END $$;
         CREATE TRIGGER test_reject_order_event BEFORE INSERT ON public.realtime_events
             FOR EACH ROW EXECUTE FUNCTION public.test_reject_order_event();",
    )
    .execute(pool)
    .await
    .expect("安装故障触发器");

    seeded
        .service()
        .mark_write_started(attempt, token, Some(binding()))
        .await
        .expect_err("事件失败则起写失败");
    assert_eq!(send_writes(pool, seeded.delivery_id).await, (0, 0));
    assert_eq!(delivery_status(pool, seeded.delivery_id).await, "claimed");
    assert_eq!(order_events(pool).await, events_before);
    let position: String = sqlx::query_scalar(
        "SELECT position.state FROM public.consignment_contracts AS contract
         JOIN public.consignment_custody_positions AS position ON position.id = contract.position_id
         WHERE contract.contract_id = $1::uuid",
    )
    .bind(CONTRACT)
    .fetch_one(pool)
    .await
    .expect("position");
    assert_eq!(position, "committed");
}

/// 轮询到有后端在等待行锁。
async fn wait_for_a_lock_wait(pool: &PgPool) {
    for _ in 0..200 {
        let waiting: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_stat_activity
             WHERE datname = current_database() AND wait_event_type = 'Lock'",
        )
        .fetch_one(pool)
        .await
        .expect("读取锁等待");
        if waiting > 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("起写没有进入受益用户行的锁等待");
}

/// 结算事务先锁受益用户、再锁履约行；起写按“受益用户 → 履约行”接线，两者交错只排队、不死锁。
#[tokio::test]
async fn postgres_consignment_dispatch_write_start_lock_order_matches_settlement_and_never_deadlocks()
 {
    let seeded = Seeded::purchased().await;
    let pool = seeded.pool().clone();
    let (attempt, token) = attempt_ready(&seeded).await;

    let mut settlement = pool.begin().await.expect("结算事务");
    sqlx::query("SELECT 1 FROM public.users WHERE id = $1::uuid FOR UPDATE")
        .bind(USER_2)
        .execute(&mut *settlement)
        .await
        .expect("结算锁受益用户");
    let service = seeded.service();
    let write_start =
        tokio::spawn(
            async move { service.mark_write_started(attempt, token, Some(binding())).await },
        );
    wait_for_a_lock_wait(&pool).await;
    sqlx::query(
        "SELECT 1 FROM public.settlement_obligation_fulfillments WHERE obligation_id = $1::uuid
         FOR UPDATE",
    )
    .bind(OBLIGATION_1)
    .execute(&mut *settlement)
    .await
    .expect("结算锁履约行（起写尚未持有它）");
    settlement.commit().await.expect("结算提交");

    let started = write_start.await.expect("起写任务").expect("起写不得死锁");
    assert!(matches!(started, WriteStart::FirstCommitted { .. }), "{started:?}");
    assert_eq!(send_writes(&pool, seeded.delivery_id).await, (1, 1));
}

/// 登记尝试（activate 等价点）同样核对受益人：改名后不登记尝试、不占 fence、零外写。
#[tokio::test]
async fn postgres_consignment_dispatch_start_attempt_retains_a_renamed_beneficiary_without_a_fence()
{
    let seeded = Seeded::purchased().await;
    let pool = seeded.pool();
    let token = Uuid::new_v4();
    seeded.claim(token, 1).await;
    set_user(pool, RENAME).await;
    let attempt = AttemptKey { delivery_id: seeded.delivery_id, fence: 1 };
    let error = seeded
        .service()
        .start_attempt(round_key(), attempt, token, frozen(PRICE))
        .await
        .expect_err("改名受益人不得登记尝试");
    assert_eq!(error, ConsignmentError::Deposit(DepositBusinessRejection::EvidenceUnproven));
    assert_eq!(send_writes(pool, seeded.delivery_id).await, (0, 0));
    let (status, fence): (String, i64) =
        sqlx::query_as("SELECT status, fencing_version FROM public.deliveries WHERE id = $1")
            .bind(seeded.delivery_id)
            .fetch_one(pool)
            .await
            .expect("投递");
    assert_eq!((status.as_str(), fence), ("claimed", 1));
}

/// MoogleMail Send 必有 Cookie 凭据：没有凭据绑定的起写失败关闭，零外写。
#[tokio::test]
async fn postgres_consignment_dispatch_write_start_without_a_credential_is_not_allowed() {
    let seeded = Seeded::purchased().await;
    let pool = seeded.pool();
    let (attempt, token) = attempt_ready(&seeded).await;
    let error = seeded
        .service()
        .mark_write_started(attempt, token, None)
        .await
        .expect_err("无凭据不得起写");
    assert_eq!(error, ConsignmentError::Deposit(DepositBusinessRejection::ActionNotAllowed));
    assert_eq!(send_writes(pool, seeded.delivery_id).await, (0, 0));
    assert_eq!(delivery_status(pool, seeded.delivery_id).await, "claimed");
}

/// 按全局顺序先锁寄售投递、再锁履约行的事务，与起写（受益用户 → 投递 → 履约行）交错：起写在投递
/// 行上排队，对方拿到履约行后提交，两边都完成、无 40P01。确定性交错：先持投递锁，等起写进入锁等待，
/// 再取履约行。
#[tokio::test]
async fn postgres_consignment_dispatch_write_start_queues_on_the_delivery_before_the_fulfillment() {
    let seeded = Seeded::purchased().await;
    let pool = seeded.pool().clone();
    let (attempt, token) = attempt_ready(&seeded).await;

    let mut settlement = pool.begin().await.expect("结算顺序事务");
    sqlx::query("SELECT 1 FROM public.deliveries WHERE id = $1 FOR UPDATE")
        .bind(seeded.delivery_id)
        .execute(&mut *settlement)
        .await
        .expect("先锁投递");
    let service = seeded.service();
    let write_start =
        tokio::spawn(
            async move { service.mark_write_started(attempt, token, Some(binding())).await },
        );
    wait_for_a_lock_wait(&pool).await;
    sqlx::query(
        "SELECT 1 FROM public.settlement_obligation_fulfillments WHERE obligation_id = $1::uuid
         FOR UPDATE",
    )
    .bind(OBLIGATION_1)
    .execute(&mut *settlement)
    .await
    .expect("再锁履约行（起写尚未持有它）");
    settlement.commit().await.expect("提交");

    let started = write_start.await.expect("起写任务").expect("起写不得死锁");
    assert!(matches!(started, WriteStart::FirstCommitted { .. }), "{started:?}");
    assert_eq!(send_writes(&pool, seeded.delivery_id).await, (1, 1));
}
