//! 退件接管的同事务效果（S2-4）：intake、返回事实、达阈值处罚与 `open_sanction` 回执、订单阶段事件全有全无；
//! 整封首次决定把可信寄售 RTS 交给同一接管写端，`return_acquired` 回执经数据库回执约束放行。
//!
//! 保护的不变量：1（重放不重复追加）、2（买家锁）、3（事实、处罚审计与事件同事务）。

use std::sync::Arc;

use auction_application::consignment::{DecideEnvelopeCommand, EnvelopeStore, ReceiptDisposition};
use auction_application::core::RequestHash;
use auction_domain::CommandId;
use auction_domain::consignment::deposit::{
    EnvelopeCapabilities, EnvelopeDecision, LockedEnvelopeFacts, TrustedConsignmentRts,
};
use auction_domain::consignment::{FeeBps, ListingPrice};
use auction_storage::{DatabaseStore, DepositRepository, DepositSettings};
use auction_time::Timestamp;
use sqlx::PgPool;
use uuid::Uuid;

use super::super::contracts::fixture::OBLIGATION_1;
use super::super::deposit::support::succeeds;
use super::super::ledger136::fixture::refund_dispatch_sql;
use super::super::refusal::fixture::{seed_intakes, submit};
use super::fixture::{Rts, original_facts, seed_original_rts, service};
use crate::support::TestDatabase;

/// (intake 数, 返回事实数, 处罚数, open_sanction 回执数, open_sanction 审计数, 订单阶段事件数)。
async fn footprint(pool: &PgPool) -> (i64, i64, i64, i64, i64, i64) {
    async fn one(pool: &PgPool, sql: &'static str) -> i64 {
        sqlx::query_scalar(sql).fetch_one(pool).await.expect("读取计数")
    }
    (
        one(pool, "SELECT count(*) FROM public.consignment_return_intakes").await,
        one(pool, "SELECT count(*) FROM public.consignment_return_facts").await,
        one(pool, "SELECT count(*) FROM public.consignment_sanctions").await,
        one(
            pool,
            "SELECT count(*) FROM public.consignment_commands
             WHERE operation = 'open_sanction' AND result_body->>'disposition' = 'sanction_pending'",
        )
        .await,
        one(
            pool,
            "SELECT count(*) FROM public.audit_records WHERE action = 'consignment.open_sanction'",
        )
        .await,
        one(
            pool,
            "SELECT count(*) FROM public.realtime_events
             WHERE kind = 'consignment.order_stage_changed'",
        )
        .await,
    )
}

/// 同一合同已有两份合格拒收；第三封（main 原件 COD 退回）接管时达阈值 3：intake、返回事实、待执行处罚、
/// `open_sanction` 回执与审计、订单阶段事件同事务提交。事件写入失败时整封回滚、零残留；重放不重复追加。
#[tokio::test]
async fn postgres_consignment_returns_acquire_commits_fact_sanction_and_event_together() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("维护账号");
    let store = Arc::new(database.store().await.expect("运行账号"));
    let pool = owner.postgres_pool();
    seed_original_rts(pool, Rts { n: 1 }).await;
    succeeds(pool, &refund_dispatch_sql()).await;
    seed_intakes(pool, 2).await;
    for n in 1..=2 {
        assert!(submit(&store, n).await.sanction.is_none(), "两份拒收尚未达阈值");
    }
    let before = footprint(pool).await;
    let service = service(&store);

    // 故障注入：最后一步（订单阶段事件）失败，接管、返回事实、处罚与回执全部回滚。
    succeeds(
        pool,
        "CREATE FUNCTION public.fixture_fail_event() RETURNS trigger LANGUAGE plpgsql AS
         $f$ BEGIN RAISE EXCEPTION '夹具：事件写入失败'; END $f$;
         CREATE TRIGGER fixture_fail_event BEFORE INSERT ON public.realtime_events
         FOR EACH ROW EXECUTE FUNCTION public.fixture_fail_event();",
    )
    .await;
    assert!(service.acquire(original_facts(Rts { n: 1 })).await.is_err(), "事件失败必须整封失败");
    assert_eq!(footprint(pool).await, before, "失败后零残留");
    succeeds(
        pool,
        "DROP TRIGGER fixture_fail_event ON public.realtime_events;
         DROP FUNCTION public.fixture_fail_event();",
    )
    .await;

    let receipt = service.acquire(original_facts(Rts { n: 1 })).await.expect("接管");
    assert!(!receipt.replayed);
    let after = footprint(pool).await;
    assert_eq!(
        after,
        (before.0 + 1, before.1 + 1, 1, 1, 1, before.5 + 1),
        "接管、第三份事实、处罚、回执、审计与事件同提交"
    );
    let (state, receipt_sanction, sanction_id): (String, String, Uuid) = sqlx::query_as(
        "SELECT sanction.state, command.result_body->>'sanction_id', sanction.id
         FROM public.consignment_sanctions AS sanction
         JOIN public.consignment_commands AS command ON command.operation = 'open_sanction'",
    )
    .fetch_one(pool)
    .await
    .expect("处罚与回执");
    assert_eq!(state, "pending");
    assert_eq!(receipt_sanction, sanction_id.to_string());

    // 重放：同 K 同命令得原回执，不追加事实、处罚、回执或事件。
    let replay = service.acquire(original_facts(Rts { n: 1 })).await.expect("重放");
    assert!(replay.replayed);
    assert_eq!(footprint(pool).await, after);
}

fn repository(store: &DatabaseStore) -> DepositRepository {
    DepositRepository::without_reservation_crypto(
        store.clone(),
        DepositSettings::new(
            FeeBps::new(500).expect("费率"),
            ListingPrice::new(ListingPrice::MIN).expect("下限"),
            ListingPrice::new(ListingPrice::MAX).expect("上限"),
            24,
            100,
        ),
    )
}

/// 整封首次决定识别出可信寄售 RTS：同事务交接管写端，回执为 `return_acquired` 并带 intake 引用，
/// 返回事实与订单阶段事件同提交；同命令重放返回原回执、零新增行。
#[tokio::test]
async fn postgres_consignment_returns_decide_envelope_hands_a_trusted_rts_to_the_acquire_writer() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("维护账号");
    let store = database.store().await.expect("运行账号");
    let pool = owner.postgres_pool();
    let rts = Rts { n: 1 };
    seed_original_rts(pool, rts).await;
    let observation = store
        .test_stored_envelope_observation(&rts.key(), rts.observation())
        .await
        .expect("读取观察")
        .expect("观察存在");
    let command = DecideEnvelopeCommand {
        command_id: CommandId::from_uuid(Uuid::from_u128(
            0x5240_0000_0000_5000_8000_0000_0000_0001,
        )),
        request_hash: RequestHash::new([0x52; 32]),
        mail_key: rts.key(),
        facts: LockedEnvelopeFacts {
            mail_key: Some(rts.key()),
            observation,
            reservations: Vec::new(),
            envelope_seller_user_id: None,
            capabilities: EnvelopeCapabilities { safe_original_return: false, take_ready: false },
            read_failure: false,
            already_decided: false,
            now: Timestamp::UNIX_EPOCH,
        },
        decision: EnvelopeDecision::AcquireReturn(TrustedConsignmentRts {
            obligation_id: Uuid::parse_str(OBLIGATION_1).expect("义务"),
            original_round: 1,
        }),
    };
    let repository = repository(&store);
    let before = footprint(pool).await;

    let receipt = repository.decide_envelope(command.clone()).await.expect("整封决定接管");
    assert_eq!(receipt.disposition, ReceiptDisposition::ReturnAcquired);
    let intake = receipt.return_intake.expect("接管引用");
    let stored: Uuid = sqlx::query_scalar("SELECT id FROM public.consignment_return_intakes")
        .fetch_one(pool)
        .await
        .expect("intake");
    assert_eq!(intake.return_intake_id, stored);
    let after = footprint(pool).await;
    assert_eq!((after.0, after.1, after.5), (before.0 + 1, before.1 + 1, before.5 + 1));

    let replay = repository.decide_envelope(command).await.expect("重放");
    assert_eq!(replay, receipt);
    assert_eq!(footprint(pool).await, after);
}
