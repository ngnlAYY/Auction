//! 订单阶段事件与 COD 实收认证同事务：事件写入失败时资金事实一并回滚，成功时事件与认证一同落库。
//!
//! 保护不变量 3：订单阶段变化的持久事件与它所描述的资金事实同提交、同回滚；同一入口在同事务里刷新
//! 买卖双方权利的动作子集，刷新失败时事件与资金事实一并回滚。

use std::time::Duration;

use auction_domain::consignment::rights::{RightRole, right_id};
use auction_storage::{DatabaseStore, PgRightsStore};

use super::super::contracts::fixture::CONTRACT;
use super::certify::Env;
use super::cod_fixture::{Shape, certify, id, ledger};

async fn events(env: &Env) -> Vec<serde_json::Value> {
    sqlx::query_scalar(
        "SELECT public_payload FROM public.realtime_events
         WHERE stream_key LIKE 'consignment_order:%' ORDER BY stream_key, stream_sequence",
    )
    .fetch_all(env.pool())
    .await
    .expect("读取订单阶段事件")
}

/// 注入事件写入失败：认证失败，资金账本、收据与履约状态逐字不变且零事件；
/// 撤掉注入后同命令原样认证成功，恰一条事件，付款阶段为 certified。
#[tokio::test]
async fn postgres_consignment_orders_certify_event_commits_and_rolls_back_with_the_funding_facts() {
    let env = Env::new(Shape::STRICT).await;
    let pool = env.pool();
    sqlx::raw_sql(sqlx::AssertSqlSafe(
        "CREATE FUNCTION public.fake_fail_order_event() RETURNS trigger
         LANGUAGE plpgsql AS $$
         BEGIN RAISE EXCEPTION 'fake injected failure'; END $$;
         CREATE TRIGGER fake_fail_order_event
         BEFORE INSERT ON public.realtime_events
         FOR EACH ROW WHEN (NEW.kind = 'consignment.order_stage_changed')
         EXECUTE FUNCTION public.fake_fail_order_event();"
            .to_owned(),
    ))
    .execute(pool)
    .await
    .expect("注入事件写入失败");

    let before = ledger(pool).await;
    let failed = env.store.test_certify_cod(certify(2, None)).await;
    assert!(failed.is_err(), "事件写入失败时认证必须失败：{failed:?}");
    assert_eq!(ledger(pool).await, before, "资金事实随事件一并回滚");
    assert!(events(&env).await.is_empty(), "回滚后不留事件");

    sqlx::raw_sql(sqlx::AssertSqlSafe(
        "DROP TRIGGER fake_fail_order_event ON public.realtime_events;".to_owned(),
    ))
    .execute(pool)
    .await
    .expect("撤掉注入");
    let receipt = env.store.test_certify_cod(certify(2, None)).await.expect("认证成功");
    assert!(!receipt.replayed);
    let events = events(&env).await;
    assert_eq!(events.len(), 1, "认证恰带一条订单阶段事件");
    assert_eq!(events[0]["payment_stage"], "certified");
    assert_eq!(events[0]["order_state"], "awaiting_handoff");
}

/// 以与生产相同的派生登记该合同的买方 `continue_order` 与卖方 `claim_payment`，状态版本 1 为 active、
/// 动作为空：代表阶段变化之前写下的旧状态，认证后必须按新阶段刷新。
async fn seed_rights(env: &Env) {
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "INSERT INTO public.consignment_rights
            (right_id, locator_digest, beneficiary_user_id, frozen_identity, world_id, kind,
             contract_id, position_id, created_command_id, created_at)
         SELECT public.c7_right_id(contract.contract_id, role.name),
                sha256(convert_to(role.name || 'order-stage-fixture', 'UTF8')),
                CASE role.name WHEN 'buyer' THEN sale.buyer_user_id ELSE contract.seller_user_id END,
                CASE role.name WHEN 'buyer' THEN sale.frozen_buyer_identity
                    ELSE contract.frozen_seller_identity END,
                contract.world_id, role.kind, contract.contract_id, NULL,
                contract.origin_command_id, contract.created_at
         FROM public.consignment_contracts AS contract
         JOIN public.consignment_sales AS sale USING (contract_id)
         CROSS JOIN (VALUES ('buyer', 'continue_order'), ('seller', 'claim_payment'))
             AS role (name, kind)
         WHERE contract.contract_id = '{CONTRACT}';
         INSERT INTO public.consignment_right_states
             (right_id, version, state, allowed_actions, occurred_at)
         SELECT right_id, 1, 'active', ARRAY[]::text[], created_at
         FROM public.consignment_rights WHERE contract_id = '{CONTRACT}';"
    )))
    .execute(env.pool())
    .await
    .expect("登记权利");
}

/// 每项权利（买方、卖方）的全部状态版本：(版本, 状态, 动作子集)，按版本升序。
type RightHistory = Vec<(i64, String, Vec<String>)>;

async fn right_history(env: &Env, role: RightRole) -> RightHistory {
    sqlx::query_as(
        "SELECT version, state, allowed_actions FROM public.consignment_right_states
         WHERE right_id = $1 ORDER BY version",
    )
    .bind(right_id(id(CONTRACT), role))
    .fetch_all(env.pool())
    .await
    .expect("读取权利状态")
}

/// 注入权利状态写入失败（事件已在同事务里追加之后）：认证失败，零事件、资金账本与两项权利的状态
/// 历史逐字不变——调用方事务回滚时，入口写下的事件与刷新一并消失。
#[tokio::test]
async fn postgres_consignment_orders_stage_change_rolls_back_event_and_rights_together() {
    let env = Env::new(Shape::STRICT).await;
    let pool = env.pool();
    seed_rights(&env).await;
    sqlx::raw_sql(sqlx::AssertSqlSafe(
        "CREATE FUNCTION public.fake_fail_right_state() RETURNS trigger
         LANGUAGE plpgsql AS $$
         BEGIN RAISE EXCEPTION 'fake injected failure'; END $$;
         CREATE TRIGGER fake_fail_right_state
         BEFORE INSERT ON public.consignment_right_states
         FOR EACH ROW EXECUTE FUNCTION public.fake_fail_right_state();"
            .to_owned(),
    ))
    .execute(pool)
    .await
    .expect("注入权利状态写入失败");

    let ledger_before = ledger(pool).await;
    let buyer_before = right_history(&env, RightRole::Buyer).await;
    let seller_before = right_history(&env, RightRole::Seller).await;
    let failed = env.store.test_certify_cod(certify(2, None)).await;
    assert!(failed.is_err(), "权利刷新失败时认证必须失败：{failed:?}");
    assert!(events(&env).await.is_empty(), "回滚后不留订单阶段事件");
    assert_eq!(ledger(pool).await, ledger_before, "资金事实随刷新失败一并回滚");
    assert_eq!(right_history(&env, RightRole::Buyer).await, buyer_before);
    assert_eq!(right_history(&env, RightRole::Seller).await, seller_before);
}

/// 认证提交后买卖双方权利按新阶段刷新：付款已认证、装备交接腿转 ready，买方追加版本 2 且可续办；
/// 卖款腿仍等交接，卖方动作仍为空，不追加无变化的版本。
#[tokio::test]
async fn postgres_consignment_orders_stage_change_refreshes_both_rights_to_the_new_stage() {
    let env = Env::new(Shape::STRICT).await;
    seed_rights(&env).await;
    let receipt = env.store.test_certify_cod(certify(2, None)).await.expect("认证成功");
    assert!(!receipt.replayed);
    assert_eq!(events(&env).await.len(), 1, "认证恰带一条订单阶段事件");
    let buyer = right_history(&env, RightRole::Buyer).await;
    assert_eq!(buyer.len(), 2, "买方追加一个新版本：{buyer:?}");
    assert_eq!(
        buyer.last().map(|(_, state, actions)| (state.as_str(), actions.clone())),
        Some(("active", vec!["continue_order".to_owned()]))
    );
    let seller = right_history(&env, RightRole::Seller).await;
    assert_eq!(seller, vec![(1, "active".to_owned(), Vec::new())], "卖方无变化不追加版本");
}

/// 未授予的 advisory 锁等待数；权利刷新事务在等待阶段变更事务释放权利锁时为 1。
async fn advisory_waiters(env: &Env) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM pg_locks WHERE locktype = 'advisory' AND NOT granted")
        .fetch_one(env.pool())
        .await
        .expect("读取锁等待")
}

/// 确定性交错：阶段变更事务 A 写完事件与权利新版本后保持未提交，此时并发的
/// `refresh_contract_rights`（B）必须等在同一把权利 advisory 锁上；A 提交后 B 读到 A 的版本、
/// 不再追加。两边都成功（无 23505 / 40P01），买方版本恰为 1、2 连续不重号，卖方不变。
#[tokio::test]
async fn postgres_consignment_orders_stage_change_and_rights_refresh_serialize_on_the_right_lock() {
    let env = Env::new(Shape::STRICT).await;
    seed_rights(&env).await;
    let contract = id(CONTRACT);
    let mut stage = env.store.postgres_pool().begin().await.expect("开启 A");
    DatabaseStore::test_record_order_stage_change(&mut stage, contract).await.expect("A 阶段变更");

    let rights = PgRightsStore::new(env.store.clone(), None);
    let refresh = tokio::spawn(async move { rights.refresh_contract_rights(contract).await });
    // B 的 lock_timeout 为 1 秒：在此之前观察到它在 advisory 锁上等待即可提交 A。
    let mut waited = false;
    for _ in 0..40 {
        if advisory_waiters(&env).await == 1 {
            waited = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(waited, "并发刷新必须等在权利 advisory 锁上，而不是越过 A 读旧版本");
    stage.commit().await.expect("提交 A");

    let appended = refresh.await.expect("B 任务").expect("B 刷新成功");
    assert_eq!(appended, 0, "B 读到 A 已写的版本，不再追加");
    let buyer = right_history(&env, RightRole::Buyer).await;
    assert_eq!(
        buyer.iter().map(|(version, _, _)| *version).collect::<Vec<_>>(),
        vec![1, 2],
        "买方版本连续不重号：{buyer:?}"
    );
    assert_eq!(buyer[1].2, vec!["continue_order".to_owned()]);
    assert_eq!(right_history(&env, RightRole::Seller).await.len(), 1);
    assert_eq!(events(&env).await.len(), 1);
}
