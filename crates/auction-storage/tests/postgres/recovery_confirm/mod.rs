//! Admin 恢复确认收口原付款履约、isekai 取消规则（见 `isekai`）、成交撤销后 Admin 投递台的
//! 重新激活门（见 `admin_retry`）、确认端把退件绑定到被收回的那次发送（见 `binding`）、
//! `equipment_recovered` 的依赖收口（见 `recovered`）与成交撤销后资产对账按恢复去向计数（见
//! `reconciliation`）的 PostgreSQL 18 用例。
//!
//! - `reclaimed` 在确认事务内把原付款履约与不再可执行的依赖履约收口为 `failed`，追加
//!   `cod_returned` 处置与审计；确认前领取的旧 claim 之后不能再推进，撤销后的义务不再被领取，
//!   命令重放不重复收口。
//! - 原付款投递仍可能外发、依赖履约已开始执行时拒绝确认，什么都不改。
//! - 领取资格按 resolution 与义务角色判定：撤销后付款与货款不再领取，`equipment_recovered`
//!   后的买方退款照常领取。
//!
//! 夹具只用直接 SQL，被测路径全部走生产端口并以运行账号提交。

#![allow(clippy::expect_used, clippy::unwrap_used)]

mod admin_retry;
mod binding;
mod fixture;
mod isekai;
mod migration;
mod reconciliation;
mod recovered;

use auction_application::auction::{AuctionLifecycleActor, AuctionLifecycleActorType};
use auction_application::operations::{
    AuditFailureMode, DispositionReadMode, OperationsAlertCollectionOverdueRead,
    OperationsAlertSourceReader,
};
use auction_application::recovery::{
    RecoveryCommandError, RecoveryCommandRepository, RecoveryConfirmationAction,
    RecoveryConfirmationCommand,
};
use auction_application::runtime_switches::{RuntimeSwitchRole, RuntimeSwitches};
use auction_application::settlement::{SettlementFulfillmentProof, SettlementFulfillmentStore};
use auction_domain::{CommandId, SettlementId};
use auction_storage::DatabaseStore;
use chrono::{Duration, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use self::fixture::{Sale, SaleSpec, insert_reclaiming_sale};
use crate::support;

fn confirmation(sale: &Sale, action: RecoveryConfirmationAction) -> RecoveryConfirmationCommand {
    let reason =
        if action == RecoveryConfirmationAction::Reclaimed { "" } else { "buyer returned" };
    RecoveryConfirmationCommand {
        command_id: CommandId::from_uuid(Uuid::new_v4()),
        recovery_id: SettlementId::from_uuid(sale.recovery_id),
        expected_state_version: sale.state_version,
        action,
        reason: reason.to_owned(),
        request_hash: RecoveryConfirmationCommand::legacy_request_hash(
            sale.recovery_id,
            sale.state_version,
            action,
            reason,
        )
        .expect("确认请求摘要"),
        request_id: None,
        actor: AuctionLifecycleActor {
            actor_type: AuctionLifecycleActorType::Admin,
            actor_id: "admin".to_owned(),
        },
    }
}

async fn fulfillment(pool: &PgPool, obligation: Uuid) -> (String, Option<String>) {
    sqlx::query_as(
        "SELECT execution_state, last_error_code
         FROM public.settlement_obligation_fulfillments WHERE obligation_id = $1",
    )
    .bind(obligation)
    .fetch_one(pool)
    .await
    .expect("读取履约")
}

async fn dispositions(pool: &PgPool, delivery_id: Uuid) -> Vec<(String, Option<String>, String)> {
    sqlx::query_as(
        "SELECT kind, reason_code, actor_source FROM public.delivery_dispositions
         WHERE delivery_id = $1 ORDER BY occurred_at",
    )
    .bind(delivery_id)
    .fetch_all(pool)
    .await
    .expect("读取处置事实")
}

async fn closure_audits(pool: &PgPool, sale: &Sale) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM public.audit_records
         WHERE action = 'auction.fulfillment.failed' AND entity_type = 'settlement_obligation'
           AND metadata->>'recovery_id' = $1",
    )
    .bind(sale.recovery_id.to_string())
    .fetch_one(pool)
    .await
    .expect("读取收口审计")
}

async fn claim_due(
    store: &DatabaseStore,
) -> Vec<auction_application::settlement::SettlementFulfillmentClaim> {
    let now = Utc::now();
    SettlementFulfillmentStore::claim_due(
        store,
        "b2-confirm-worker".to_owned(),
        now,
        now + Duration::minutes(5),
        20,
    )
    .await
    .expect("领取履约")
}

/// 确认 reclaimed 同事务收口原付款与卖方货款，买方退款保持 contingent；旧 claim 失败关闭，
/// 撤销后不再领取；重放不重复收口。
#[tokio::test]
async fn postgres_reclaim_confirmation_closes_payment_and_dependents() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let sale = insert_reclaiming_sale(
        pool,
        SaleSpec {
            world: "main",
            sequence: 7_500_001,
            cod_return_count: 3,
            cod_return_limit: 3,
            blacklist_satisfied: true,
        },
    )
    .await;

    // 确认之前 Worker 已领取原付款（enqueued 等待 Sent 证据）。
    let stale = claim_due(&store)
        .await
        .into_iter()
        .find(|claim| claim.obligation_id.as_uuid() == sale.payment)
        .expect("确认前原付款仍可被领取");

    let command = confirmation(&sale, RecoveryConfirmationAction::Reclaimed);
    let result = store.confirm_recovery(command.clone()).await.expect("确认 reclaimed");
    assert_eq!(result.resolution, "reclaimed");
    assert_eq!(result.stage, "relist_pending");

    assert_eq!(
        fulfillment(pool, sale.payment).await,
        ("failed".to_owned(), Some("recovery_reclaimed".to_owned()))
    );
    assert_eq!(
        fulfillment(pool, sale.seller).await,
        ("failed".to_owned(), Some("recovery_sale_voided".to_owned()))
    );
    assert_eq!(fulfillment(pool, sale.refund).await, ("pending".to_owned(), None));
    assert_eq!(
        dispositions(pool, sale.payment_delivery).await,
        vec![(
            "cod_returned".to_owned(),
            Some("recovery_reclaimed".to_owned()),
            "admin".to_owned()
        )]
    );
    assert_eq!(closure_audits(pool, &sale).await, 2);
    let (status,): (String,) = sqlx::query_as("SELECT status FROM public.deliveries WHERE id = $1")
        .bind(sale.payment_delivery)
        .fetch_one(pool)
        .await
        .expect("读取原付款投递");
    assert_eq!(status, "succeeded", "原投递与发送证明不改写");

    // 旧 claim 不能再满足或退避：版本与 fence 已变化。
    let proof = SettlementFulfillmentProof {
        kind: "mooglemail_sent_collected".to_owned(),
        reference: "1234567".to_owned(),
        digest: [7; 32],
        observed_at: Utc::now(),
    };
    assert!(store.satisfy(stale.clone(), proof).await.is_err(), "撤销后旧 claim 不能满足付款");
    assert!(
        store.fail(stale, Duration::seconds(30), "evidence_not_observed".to_owned()).await.is_err(),
        "撤销后旧 claim 不能写回退避"
    );
    assert!(
        claim_due(&store)
            .await
            .iter()
            .all(|claim| ![sale.payment, sale.seller].contains(&claim.obligation_id.as_uuid())),
        "撤销后的付款与货款不再领取"
    );

    // 告警分类口径：两条收口失败都由 cod_returned 处置解除；迁移前口径仍计入（影子期差异）。
    for (mode, expected) in [(DispositionReadMode::Classified, 0), (DispositionReadMode::Legacy, 2)]
    {
        let snapshot = store
            .clone()
            .with_runtime_switches(
                RuntimeSwitches::new(RuntimeSwitchRole::Worker, AuditFailureMode::Shadow, true)
                    .with_disposition_read_mode_baseline(mode),
            )
            .snapshot(Utc::now(), OperationsAlertCollectionOverdueRead::Skip)
            .await
            .expect("读取告警快照");
        assert_eq!(snapshot.fulfillment.expect("履约水位").failed, expected, "{mode:?}");
    }

    // 同一命令重放返回原结果，不重复收口、处置或审计。
    let replayed = store.confirm_recovery(command).await.expect("重放确认");
    assert_eq!(replayed.state_version, result.state_version);
    assert_eq!(dispositions(pool, sale.payment_delivery).await.len(), 1);
    assert_eq!(closure_audits(pool, &sale).await, 2);

    store.postgres_pool().close().await;
}

/// 反例：原付款投递仍在发送队列（外部写入未解决）或卖方货款已开始执行时，确认失败关闭且
/// 不改写任何履约、处置或恢复记录。
#[tokio::test]
async fn postgres_reclaim_confirmation_rejects_unresolved_writes() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    for (sequence, setup) in [
        (
            7_500_011,
            "UPDATE public.deliveries SET status = 'pending', completed_at = NULL,
                 external_reference = NULL WHERE id = $1",
        ),
        (
            7_500_012,
            "UPDATE public.settlement_obligation_fulfillments
             SET execution_state = 'prepared', delivery_id = $1,
                 prepare_command_id = gen_random_uuid(),
                 prepare_request_digest = decode(repeat('b1', 32), 'hex')
             WHERE obligation_id = $2",
        ),
    ] {
        let sale = insert_reclaiming_sale(
            pool,
            SaleSpec {
                world: "main",
                sequence,
                cod_return_count: 2,
                cod_return_limit: 2,
                blacklist_satisfied: true,
            },
        )
        .await;
        if sequence == 7_500_011 {
            sqlx::query(sqlx::AssertSqlSafe(setup))
                .bind(sale.payment_delivery)
                .execute(pool)
                .await
                .expect("模拟仍在队列中的原付款投递");
        } else {
            let other_delivery = Uuid::new_v4();
            sqlx::query(
                "INSERT INTO public.deliveries
                    (id, command_id, request_digest, origin_kind, origin_id, recipient, payload,
                     status)
                 VALUES ($1, gen_random_uuid(), decode(repeat('b2', 32), 'hex'), 'settlement',
                         $2, 'Recovery seller', '{\"operation\":\"send\"}'::jsonb, 'prepared')",
            )
            .bind(other_delivery)
            .bind(sale.seller.to_string())
            .execute(pool)
            .await
            .expect("写入卖方货款投递");
            sqlx::query(sqlx::AssertSqlSafe(setup))
                .bind(other_delivery)
                .bind(sale.seller)
                .execute(pool)
                .await
                .expect("模拟已开始执行的卖方货款");
        }
        let error = store
            .confirm_recovery(confirmation(&sale, RecoveryConfirmationAction::Reclaimed))
            .await
            .expect_err("存在未解决外部写入时不能自动收口");
        assert!(
            matches!(
                error,
                auction_application::core::ApplicationError::RecoveryCommand(
                    RecoveryCommandError::NotConfirmable
                )
            ),
            "{error:?}"
        );
        assert_eq!(fulfillment(pool, sale.payment).await.0, "enqueued");
        assert!(dispositions(pool, sale.payment_delivery).await.is_empty());
        let (stage, resolution): (String, Option<String>) = sqlx::query_as(
            "SELECT stage, resolution FROM public.settlement_recoveries WHERE id = $1",
        )
        .bind(sale.recovery_id)
        .fetch_one(pool)
        .await
        .expect("读取恢复记录");
        assert_eq!((stage.as_str(), resolution), ("reclaiming", None));
    }

    store.postgres_pool().close().await;
}

/// 领取资格按义务角色与 resolution 判定。成交撤销（reclaimed）后付款与货款不再领取；
/// `equipment_recovered` 之后买方退款照常领取，交付义务不再领取。
#[tokio::test]
async fn postgres_resolution_gates_claims_but_keeps_recovered_refunds() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let recovered = insert_reclaiming_sale(
        pool,
        SaleSpec {
            world: "isekai",
            sequence: 7_500_021,
            cod_return_count: 1,
            cod_return_limit: 3,
            blacklist_satisfied: false,
        },
    )
    .await;
    let handoff = recovered.handoff.expect("isekai 夹具带交付义务");
    // 直接构造 equipment_recovered 之后的事实：载体已付款、交付就绪、退款已激活。
    sqlx::query(
        "UPDATE public.settlement_obligation_fulfillments
         SET execution_state = 'satisfied', proof_kind = 'mooglemail_sent_collected',
             proof_reference = '1234567', proof_digest = decode(repeat('c1', 32), 'hex'),
             proof_observed_at = clock_timestamp(), completed_at = clock_timestamp()
         WHERE obligation_id = $1",
    )
    .bind(recovered.payment)
    .execute(pool)
    .await
    .expect("构造载体已付款");
    // 退款激活与清除或有未激活标记同事务，与恢复确认的激活写法一致。
    sqlx::query(
        "WITH activated AS (
             UPDATE public.settlement_obligations SET business_state = 'ready'
             WHERE id = ANY($1) RETURNING id
         )
         UPDATE public.settlement_obligation_fulfillments SET dormant = false
         WHERE obligation_id IN (SELECT id FROM activated)",
    )
    .bind(vec![handoff, recovered.refund])
    .execute(pool)
    .await
    .expect("构造交付就绪与退款激活");
    sqlx::query(
        "UPDATE public.settlement_recoveries
         SET stage = 'refunding', resolution = 'equipment_recovered',
             relisted_auction_id = auction_id, relisted_lot_id = lot_id
         WHERE id = $1",
    )
    .bind(recovered.recovery_id)
    .execute(pool)
    .await
    .expect("构造 equipment_recovered");

    let claimed: Vec<Uuid> =
        claim_due(&store).await.iter().map(|claim| claim.obligation_id.as_uuid()).collect();
    assert!(claimed.contains(&recovered.refund), "equipment_recovered 后的退款必须照常领取");
    assert!(!claimed.contains(&handoff), "装备已收回重列，交付不能再领取");
    assert!(!claimed.contains(&recovered.seller));

    store.postgres_pool().close().await;
}
