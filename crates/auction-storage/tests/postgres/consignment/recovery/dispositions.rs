//! 退回处置（7A-2 resolve_return）：同一消费只产生一次处置，重放返回原处置，冻结配置异请求冲突；
//! 退款腿被退不触发再售，可交付性不明留案。
//!
//! 保护的不变量：1（命令幂等）、6（失败关闭）、8（只追加）、10（冻结配置）。

use std::sync::Arc;

use auction_application::consignment::{
    AutoRelistSetting, ConsignmentError, ReturnDispositionService, ReturnDispositionStore,
};
use auction_domain::consignment::recovery::{
    BuyerRightEffect, ListingEffect, RefundEffect, RelistEligibility,
};
use auction_storage::DatabaseStore;
use sqlx::PgPool;

use super::super::contracts::fixture::CONTRACT;
use super::super::returns::fixture::{
    Rts, SIGNED_VALIDATOR, consume_input, seed_consumed, seed_original_started,
    seed_payout_started, service as return_service,
};
use crate::support::TestDatabase;

const SETTING_OFF: AutoRelistSetting = AutoRelistSetting { enabled: false, config_digest: [9; 32] };
const SETTING_ON: AutoRelistSetting = AutoRelistSetting { enabled: true, config_digest: [9; 32] };

async fn setup(database: &TestDatabase) -> (Arc<DatabaseStore>, DatabaseStore) {
    let owner = database.maintenance_store().await.expect("维护账号");
    let store = Arc::new(database.store().await.expect("运行账号"));
    (store, owner)
}

fn service(store: &Arc<DatabaseStore>) -> ReturnDispositionService {
    let port: Arc<dyn ReturnDispositionStore> = store.clone();
    ReturnDispositionService::new(port)
}

async fn count(pool: &PgPool, sql: &'static str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(pool).await.expect("读取计数")
}

/// 退款腿被退：恢复原 refund 用途、无再售；同一消费只产生一次处置，重放返回原处置，
/// 冻结配置不同是异请求，且不改写已作的决定。
#[tokio::test]
async fn postgres_consignment_recovery_resolve_return_records_one_disposition_per_consumption() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let (store, owner) = setup(&database).await;
    let pool = owner.postgres_pool();
    seed_consumed(pool, &store, Rts { n: 1 }).await;
    let service = service(&store);

    let pending = service.pending(None, 10).await.expect("待处置").items;
    assert_eq!(pending.len(), 1);
    // 键集分页：读满一页给出游标，游标之后没有更多待办（正例）；读不满一页没有游标（反例）。
    let page = service.pending(None, 1).await.expect("满页");
    assert_eq!(page.items, pending);
    let next = page.next.expect("读满一页应给出游标");
    let after = service.pending(Some(next), 1).await.expect("游标之后");
    assert!((after.items.is_empty(), after.next.is_none()) == (true, true), "{after:?}");
    assert_eq!(service.pending(None, 10).await.expect("未满页").next, None);
    let first = service.resolve(pending[0], SETTING_OFF).await.expect("处置");
    assert!(!first.replayed);
    assert_eq!(first.decision.refund, RefundEffect::RestoreRefundPurpose);
    assert_eq!(first.decision.listing, ListingEffect::Unchanged);
    assert_eq!(first.decision.relist, RelistEligibility::Forbidden, "退款被退不触发再售");
    assert_eq!(count(pool, "SELECT count(*) FROM public.consignment_return_dispositions").await, 1);
    assert!(
        service.pending(None, 10).await.expect("待处置").items.is_empty(),
        "已处置的消费不再待办"
    );

    // 正例：同请求重放返回原处置，不新增行。
    let replay = service.resolve(pending[0], SETTING_OFF).await.expect("重放");
    assert!(replay.replayed);
    assert_eq!(replay.disposition_id, first.disposition_id);
    assert_eq!(count(pool, "SELECT count(*) FROM public.consignment_return_dispositions").await, 1);

    // 反例：同一消费换了冻结配置是异请求，不得另造处置，也不得改写已冻结的开关。
    let conflict = service.resolve(pending[0], SETTING_ON).await.expect_err("异请求必须冲突");
    assert!(matches!(conflict, ConsignmentError::CommandIdConflict));
    assert_eq!(count(pool, "SELECT count(*) FROM public.consignment_return_dispositions").await, 1);
    assert_eq!(
        count(
            pool,
            "SELECT count(*) FROM public.consignment_return_dispositions
             WHERE NOT auto_relist_enabled"
        )
        .await,
        1,
        "决策时冻结的开关值不变"
    );

    // 退款被退没有任何再售动作：无重列决定、无新代，挂单仍是成交待履约。
    assert_eq!(count(pool, "SELECT count(*) FROM public.consignment_relist_decisions").await, 0);
    assert_eq!(
        count(pool, "SELECT count(*) FROM public.consignment_disposition_generations").await,
        1
    );
    assert_eq!(
        count(
            pool,
            "SELECT count(*) FROM public.consignment_listings WHERE state = 'sale_pending'"
        )
        .await,
        1
    );
}

/// 缺配置摘要失败关闭：不写任何行，不占命令 ID。
#[tokio::test]
async fn postgres_consignment_recovery_resolve_return_fails_closed_without_a_config_digest() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let (store, owner) = setup(&database).await;
    let pool = owner.postgres_pool();
    seed_consumed(pool, &store, Rts { n: 1 }).await;
    let service = service(&store);
    let pending = service.pending(None, 10).await.expect("待处置").items;

    let error = service
        .resolve(pending[0], AutoRelistSetting { enabled: true, config_digest: [0; 32] })
        .await
        .expect_err("没有配置摘要不得处置");
    assert!(matches!(error, ConsignmentError::Unavailable));
    assert_eq!(count(pool, "SELECT count(*) FROM public.consignment_return_dispositions").await, 0);
    assert_eq!(
        count(
            pool,
            "SELECT count(*) FROM public.consignment_commands WHERE operation = 'resolve_return'"
        )
        .await,
        0
    );
}

/// main 未付原件被退：接管已同事务写下合格返回事实（S2-4），未达冻结上限时保留合同（D9 §5.1 main 行），
/// 零资金动作、禁止再售，即使冻结开关为开。没有返回事实时的留案分支由领域矩阵单测覆盖。
#[tokio::test]
async fn postgres_consignment_recovery_resolve_return_retains_the_contract_below_the_limit() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let (store, owner) = setup(&database).await;
    let pool = owner.postgres_pool();
    let (receipt, claim) = seed_original_started(pool, &store, Rts { n: 1 }).await;
    return_service(&store)
        .consume(consume_input(&receipt, Rts { n: 1 }, claim, SIGNED_VALIDATOR, 0xa7))
        .await
        .expect("消费");
    let service = service(&store);
    let pending = service.pending(None, 10).await.expect("待处置").items;
    assert_eq!(pending.len(), 1);

    let resolved = service.resolve(pending[0], SETTING_ON).await.expect("处置");
    assert_eq!(resolved.decision.buyer_right, BuyerRightEffect::Retain);
    assert_eq!(resolved.decision.refund, RefundEffect::None);
    assert_eq!(resolved.decision.listing, ListingEffect::Unchanged);
    assert_eq!(resolved.decision.relist, RelistEligibility::Forbidden, "保留合同禁止再售");
    // 反例：保留合同的处置没有任何再售资格，自动重列的待办里不出现它。
    assert_eq!(count(pool, "SELECT count(*) FROM public.consignment_relist_decisions").await, 0);
}

/// 原件“精确收回”以 6D5 的消费事实判定，不以 position 状态代理（D9 §5.1）。
/// 反例：position 看似在库（isekai 未付载体的原物从未离库，此处用同样“在库”的 position），
/// 但合同没有原件腿的消费事实（只有一次退款腿消费）时不放行；正例：同一合同经精确 Take 消费原件腿后成立。
#[tokio::test]
async fn postgres_consignment_recovery_original_taken_follows_the_consumption_fact_not_the_position()
 {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let (store, owner) = setup(&database).await;
    let pool = owner.postgres_pool();
    seed_consumed(pool, &store, Rts { n: 1 }).await;
    let contract = CONTRACT.parse().expect("合同 ID");
    let position_state: String =
        sqlx::query_scalar("SELECT state FROM public.consignment_custody_positions")
            .fetch_one(pool)
            .await
            .expect("position 状态");
    assert!(
        matches!(position_state.as_str(), "in_custody" | "committed"),
        "前提：position 看似在库，旧的代理判定会放行"
    );
    assert!(!store.test_original_exactly_taken(contract).await.expect("读取"));

    let database = TestDatabase::create().await.expect("克隆模板库");
    let (store, owner) = setup(&database).await;
    let pool = owner.postgres_pool();
    let (receipt, claim) = seed_original_started(pool, &store, Rts { n: 1 }).await;
    assert!(!store.test_original_exactly_taken(contract).await.expect("读取"), "起写未消费不算");
    return_service(&store)
        .consume(consume_input(&receipt, Rts { n: 1 }, claim, SIGNED_VALIDATOR, 0xa7))
        .await
        .expect("消费");
    assert!(store.test_original_exactly_taken(contract).await.expect("读取"));
}

/// 卖款腿被退（E20）：精确 Take 消费后的处置只恢复原受益权——零退款、挂单不动、禁止再售，即使冻结开关为开；
/// 没有重列决定、没有新代，挂单仍是成交待履约。正例（未付收回在开关开启时重列）见 `relist.rs`。
#[tokio::test]
async fn postgres_consignment_recovery_resolve_return_seller_payout_never_relists() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let (store, owner) = setup(&database).await;
    let pool = owner.postgres_pool();
    let (receipt, claim) = seed_payout_started(pool, &store, Rts { n: 1 }).await;
    return_service(&store)
        .consume(consume_input(&receipt, Rts { n: 1 }, claim, SIGNED_VALIDATOR, 0xa7))
        .await
        .expect("消费");
    let service = service(&store);
    let pending = service.pending(None, 10).await.expect("待处置").items;
    assert_eq!(pending.len(), 1);

    let resolved = service.resolve(pending[0], SETTING_ON).await.expect("处置");
    assert_eq!(resolved.decision.buyer_right, BuyerRightEffect::NotApplicable);
    assert_eq!(resolved.decision.refund, RefundEffect::None);
    assert_eq!(resolved.decision.listing, ListingEffect::Unchanged);
    assert_eq!(resolved.decision.relist, RelistEligibility::Forbidden, "卖款被退不触发再售");
    assert_eq!(count(pool, "SELECT count(*) FROM public.consignment_relist_decisions").await, 0);
    assert_eq!(
        count(pool, "SELECT count(*) FROM public.consignment_disposition_generations").await,
        1
    );
    assert_eq!(
        count(
            pool,
            "SELECT count(*) FROM public.consignment_listings WHERE state = 'sale_pending'"
        )
        .await,
        1
    );
}
