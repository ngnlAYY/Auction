//! 退款类处置的证据归属与证据查询：别的合同的真实证据不能给本合同退款；退件消费证据查询在通过路径上执行到；
//! 买家已确领原件时证据互相矛盾，保留给人工，零资金变化。
//!
//! 夹具全部走真实写序与全部守卫（不关任何触发器、外键）。“别的合同”由第二个真实的代管 position 与合同图构成：
//! 复用第一个 position 的夹具文本，把全部业务 ID、MID 与装备号映射为另一组（`other`），所以每条语句都通过同一套守卫。
//! `refund_season` 用 isekai 的整条合同夹具（`funding/cod_fixture`：存入、购买、付款认证、季末），只激活 isekai 邮件凭据。

use auction_application::consignment::{DepositBusinessRejection, OrderResolveAction};
use sqlx::PgPool;
use uuid::Uuid;

use super::super::contracts::fixture::{
    CONTRACT, Graph, OBLIGATION_1, OBLIGATION_2, OBLIGATION_3, OBLIGATION_4, WINDOW,
};
use super::super::deposit::support::{
    ASSET, ATTEMPT, DECIDE, DECISION, DELIVERY, DELIVERY_COMMAND, ENVELOPE, ISSUE, ITEM, LISTING,
    OBSERVATION, POSITION, PROOF as MAIL_PROOF, PROOF_COMMAND, PUBLISH, REGISTER, RESERVATION,
    SNAPSHOT, WORK, accept_listing, attempt_start_proof_and_custody, consume_reservation,
    custody_event, decide_command, delivery_and_work, initial_and_final, item_position_claim,
    listing_awaiting_deposit, observation_and_asset, reservation_issued, succeeds,
};
use super::super::funding::cod_fixture::{Shape, certify, seed as seed_isekai_sale};
use super::super::ledger136::fixture::{
    AVAILABLE_COMMAND, PAY_ATTEMPT, PAY_DELIVERY, PAY_MID, PAY_ROUND, PRICE, PURCHASE_COMMAND,
    PURCHASED_AT, available_sql, original_dispatch_sql, purchase_command, purchase_listing,
    purchase_position, purchase_sql,
};
use super::super::ledger136::purchase::{buyer_claimed_original_from, buyer_claimed_original_sql};
use super::super::recovery::sale_recovery::refund_call::{closures, fund_the_sale};
use super::super::recovery::{PROOF, command_ids, proof_sql};
use super::super::returns::fixture::{
    Claim, Rts, SIGNED_VALIDATOR, claim_take, consume_input, original_facts, seed_original_started,
    service as return_service, start, take_delivery,
};
use super::effects::current_version;
use super::resolve::{contract, funds, order_events, paid_sale_with_loss_proof, resolutions};
use super::{command, count, ledger_of, rejection, resolve_input, service};
use crate::support::TestDatabase;

const OTHER_PROOF: &str = "7e730000-0000-4000-8000-000000000001";
const OTHER_MARK: &str = "b";

/// 把夹具文本里的业务 ID 映射成“另一组”：UUID 倒数第二位换成 `b`，MID 555 换成 5551，装备号 42 换成 43。
/// 用户、凭据、会话与重放密钥是共享的，不映射。
fn other(sql: &str) -> String {
    let ids = [
        LISTING,
        SNAPSHOT,
        REGISTER,
        PUBLISH,
        DECIDE,
        ISSUE,
        RESERVATION,
        ENVELOPE,
        OBSERVATION,
        ASSET,
        DECISION,
        ITEM,
        POSITION,
        DELIVERY,
        DELIVERY_COMMAND,
        WORK,
        ATTEMPT,
        MAIL_PROOF,
        PROOF_COMMAND,
        CONTRACT,
        OBLIGATION_1,
        OBLIGATION_2,
        OBLIGATION_3,
        OBLIGATION_4,
        "86000000-0000-0000-0000-000000000001",
        AVAILABLE_COMMAND,
        PURCHASE_COMMAND,
        PAY_DELIVERY,
        PAY_ROUND,
        PAY_ATTEMPT,
    ];
    // send_sql 的命令 ID 由 delivery ID 派生，也要映射。
    let derived_command = format!("c71{}", &PAY_DELIVERY[3..]);
    let mut mapped = sql.to_owned();
    for id in ids.into_iter().chain([derived_command.as_str()]) {
        mapped = mapped.replace(id, &format!("{}{OTHER_MARK}{}", &id[..34], &id[35..]));
    }
    mapped = distinct_digests(&mapped);
    mapped
        .replace(&format!("'{PAY_MID}'"), "'9011'")
        .replace("555", "5551")
        .replace("'42'", "'43'")
        .replace("\"42\"", "\"43\"")
}

/// 夹具里的摘要是 `repeat('NN', 32)` 的固定字节，部分列有唯一约束（如预约令牌摘要）：把每个字节取反最高位，
/// 同一字节在各语句里仍映射到同一值，所以跨表引用一致。
fn distinct_digests(sql: &str) -> String {
    const MARK: &str = "repeat('";
    let mut out = String::with_capacity(sql.len());
    let mut rest = sql;
    while let Some(at) = rest.find(MARK) {
        let (head, tail) = rest.split_at(at + MARK.len());
        out.push_str(head);
        let (byte, after) = tail.split_at(2);
        match u8::from_str_radix(byte, 16) {
            Ok(value) => out.push_str(&format!("{:02x}", value ^ 0x80)),
            Err(_) => out.push_str(byte),
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

/// 第二个真实的已购 main 件：代管在库、上架、被买家购买（Sale 合同图、position committed、挂单 sale_pending）。
async fn seed_other_purchased(pool: &PgPool) {
    let custody = listing_awaiting_deposit()
        + &reservation_issued()
        + &observation_and_asset()
        + &decide_command()
        + &initial_and_final()
        + &item_position_claim()
        + &consume_reservation()
        + &accept_listing()
        + &delivery_and_work()
        + &custody_event();
    // 与 seed_main_custody、seed_purchased 相同的分步：接受的延迟约束要求提交时仍是 awaiting_take。
    succeeds(pool, &other(&custody)).await;
    succeeds(pool, &other(&attempt_start_proof_and_custody())).await;
    succeeds(pool, &other(&available_sql())).await;
    succeeds(pool, &other(&purchase_sql())).await;
}

/// 第二个合同的原件腿已首派、退回邮件已到、接管并起写后经真实写端消费；返回它的退件消费 ID。
async fn other_consumption(
    pool: &PgPool,
    store: &std::sync::Arc<auction_storage::DatabaseStore>,
) -> Uuid {
    let in_transit = format!(
        "UPDATE public.consignment_custody_positions
         SET state = 'in_transit', version = 4, updated_at = '{PURCHASED_AT}'
         WHERE id = '{POSITION}';
         INSERT INTO public.consignment_custody_events
            (position_id, version, command_id, event_kind, from_state, to_state, proof_id,
             listing_id, listing_version, occurred_at)
         VALUES ('{POSITION}', 4, '{AVAILABLE_COMMAND}', 'in_transit', 'committed',
                 'in_transit', NULL, '{LISTING}', 6, '{PURCHASED_AT}');"
    );
    succeeds(pool, &other(&(original_dispatch_sql() + &in_transit))).await;
    let rts = Rts { n: 2 };
    succeeds(pool, &rts.sql(0, 101)).await;
    let mut facts = original_facts(rts);
    facts.contract_id = Uuid::parse_str(&other(CONTRACT)).expect("合同");
    facts.obligation_id = Uuid::parse_str(&other(OBLIGATION_1)).expect("义务");
    facts.original_attempt.delivery_id = Uuid::parse_str(&other(PAY_DELIVERY)).expect("投递");
    facts.sent_mid = "9011".to_owned();
    let service = return_service(store);
    let receipt = service.acquire(facts).await.expect("接管");
    let claim = Claim::new(22, 1, true);
    claim_take(pool, &receipt, take_delivery(pool, &receipt).await, claim).await;
    start(&service, pool, &receipt, claim).await.expect("起写");
    service
        .consume(consume_input(&receipt, rts, claim, SIGNED_VALIDATOR, 0xa8))
        .await
        .expect("消费");
    sqlx::query_scalar(
        "SELECT id FROM public.consignment_return_consumptions WHERE contract_id = $1",
    )
    .bind(Uuid::parse_str(&other(CONTRACT)).expect("合同"))
    .fetch_one(pool)
    .await
    .expect("别的合同的消费")
}

/// 别的合同的真实灭失证据（第二个 position 上的损失证明）。
async fn other_loss_proof(pool: &PgPool) -> Uuid {
    let commands = command_ids(pool, 1).await;
    succeeds(pool, &other(&proof_sql(OTHER_PROOF, "fake-observation-other", &commands[0]))).await;
    Uuid::parse_str(OTHER_PROOF).expect("证据")
}

/// 反例：`refund_lost` 用属于别的合同的真实灭失证明：确定拒绝 `evidence_unproven`，零资金痕迹。
/// 去掉证据查询里的合同绑定，别的合同的证明就能给本合同退款，本用例随之失败。
#[tokio::test]
async fn postgres_consignment_order_resolve_refuses_loss_evidence_of_another_contract() {
    let (_database, pool, runtime) = paid_sale_with_loss_proof().await;
    seed_other_purchased(&pool).await;
    let foreign = other_loss_proof(&pool).await;
    let service = service(runtime);

    let error = service
        .resolve(resolve_input(1, contract(), 1, OrderResolveAction::RefundLost, foreign))
        .await
        .expect_err("别的合同的证据不能退款");

    assert_eq!(rejection(&error), Some(DepositBusinessRejection::EvidenceUnproven));
    assert_eq!(ledger_of(&pool, command(1)).await, Some((409, "evidence_unproven".to_owned())));
    assert_eq!(funds(&pool).await, (0, 96, 5), "没有任何资金变化");
    assert_eq!((closures(&pool).await, resolutions(&pool).await), (0, 0));
    assert_eq!(order_events(&pool).await, 0);
    // 对照：本合同自己的灭失证明在同一个库里照常通过。
    let own = service
        .resolve(resolve_input(
            2,
            contract(),
            1,
            OrderResolveAction::RefundLost,
            Uuid::parse_str(PROOF).expect("证据"),
        ))
        .await;
    assert!(own.is_ok(), "本合同的证据应通过：{own:?}");
    assert_eq!(funds(&pool).await, (101, 0, 0));
}

/// 正例：退件消费证据（原件已精确收回并经真实写端消费）经 6B 写端全额退款；证据查询在通过路径上执行到。
#[tokio::test]
async fn postgres_consignment_order_resolve_refund_returned_accepts_its_own_consumption() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    let store = std::sync::Arc::new(database.store().await.expect("以运行账号连接"));
    let rts = Rts { n: 1 };
    let (receipt, claim) = seed_original_started(pool, &store, rts).await;
    return_service(&store)
        .consume(consume_input(&receipt, rts, claim, SIGNED_VALIDATOR, 0xa7))
        .await
        .expect("消费");
    fund_the_sale(&store).await;
    let consumption: Uuid =
        sqlx::query_scalar("SELECT id FROM public.consignment_return_consumptions")
            .fetch_one(pool)
            .await
            .expect("消费");
    let service = service(database.store().await.expect("运行账号"));

    // 接管与消费已追加订单阶段事件，订单版本随之前进：按当前版本提交。
    let version = current_version(pool).await;
    service
        .resolve(resolve_input(
            1,
            contract(),
            version,
            OrderResolveAction::RefundReturned,
            consumption,
        ))
        .await
        .expect("退款");

    assert_eq!(funds(pool).await, (101, 0, 0));
    assert_eq!((closures(pool).await, resolutions(pool).await), (1, 1));
    assert_eq!(
        count(
            pool,
            "SELECT count(*) FROM public.consignment_order_resolutions WHERE action = 'refund_returned'"
        )
        .await,
        1
    );
}

/// 反例：`refund_returned` 用属于别的合同的真实退件消费：确定拒绝 `evidence_unproven`，零资金痕迹。
/// 去掉证据查询里的合同绑定，别的合同的消费就能给本合同退款，本用例随之失败。
#[tokio::test]
async fn postgres_consignment_order_resolve_refuses_a_return_consumption_of_another_contract() {
    let (database, pool, _runtime) = paid_sale_with_loss_proof().await;
    seed_other_purchased(&pool).await;
    let store = std::sync::Arc::new(database.store().await.expect("运行账号"));
    let foreign = other_consumption(&pool, &store).await;
    // 别的合同的接管与消费会在它自己的流里追加订单阶段事件；这里只核对本次拒绝不再追加。
    let events = order_events(&pool).await;
    let service = service(database.store().await.expect("运行账号"));

    let error = service
        .resolve(resolve_input(1, contract(), 1, OrderResolveAction::RefundReturned, foreign))
        .await
        .expect_err("别的合同的消费不能退款");

    assert_eq!(rejection(&error), Some(DepositBusinessRejection::EvidenceUnproven));
    assert_eq!(funds(&pool).await, (0, 96, 5), "没有任何资金变化");
    assert_eq!((closures(&pool).await, resolutions(&pool).await), (0, 0));
    assert_eq!(order_events(&pool).await, events);
}

/// isekai 的已付成交（整条合同都在 isekai：存入、购买、原付款 Send、`certify_cod` 认证付款），季窗的交割界已过；
/// 只激活 isekai 的邮件凭据。夹具是 `funding/cod_fixture` 的真实写法，没有任何放宽。
async fn isekai_paid_sale() -> (crate::support::TestDatabase, PgPool) {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool().clone();
    seed_isekai_sale(&pool, Shape { season_ended: true, ..Shape::STRICT }).await;
    database
        .store()
        .await
        .expect("运行账号")
        .test_certify_cod(certify(2, None))
        .await
        .expect("认证付款");
    (database, pool)
}

/// 季末终态：季窗已过，position 从 committed 终结并释放占用（与 `contracts/season_end.rs` 同样的写法，
/// 守卫全部生效）。
fn season_end_sql() -> String {
    format!(
        "UPDATE public.consignment_custody_positions
         SET state = 'season_ended', version = 4, updated_at = transaction_timestamp()
         WHERE id = '{POSITION}';
         INSERT INTO public.consignment_custody_events
            (position_id, version, command_id, event_kind, from_state, to_state, proof_id,
             listing_id, listing_version, occurred_at)
         VALUES ('{POSITION}', 4, '{PROOF_COMMAND}', 'season_ended', 'committed',
                 'season_ended', NULL, '{LISTING}', 6, transaction_timestamp());
         UPDATE public.consignment_equipment_claims
         SET reservation_id = NULL, position_id = NULL, revision = revision + 1,
             changed_at = transaction_timestamp(), last_fact_id = '{POSITION}'
         WHERE world_id = 'isekai' AND canonical_eid = '42';"
    )
}

/// 第二个真实的 isekai 件：同一季窗、同一 isekai 邮件会话，代管在库后被购买并季末终结。
async fn seed_other_season_ended(pool: &PgPool) {
    let custody = listing_awaiting_deposit()
        + &reservation_issued()
        + &observation_and_asset()
        + &decide_command()
        + &initial_and_final()
        + &item_position_claim()
        + &consume_reservation()
        + &accept_listing()
        + &delivery_and_work()
        + &custody_event();
    // 与 seed_isekai_awaiting 相同的世界替换：邮件键、预约与 position 改为 isekai 并绑定季窗。
    let isekai = custody.replace("'main'", "'isekai'").replace("|main|", "|isekai|").replace(
        " 'isekai', '42',\n                 NULL, 'awaiting_take'",
        &format!(" 'isekai', '42',\n                 '{WINDOW}', 'awaiting_take'"),
    );
    assert!(isekai.contains(WINDOW), "isekai 夹具必须绑定季窗");
    succeeds(pool, &other(&isekai)).await;
    succeeds(pool, &other(&attempt_start_proof_and_custody())).await;
    succeeds(pool, &other(&available_sql())).await;
    let graph = Graph {
        price: PRICE,
        fee: 5,
        seller_amount: 96,
        payout_amount: 96,
        ..Graph::sale_isekai()
    }
    .sql(PURCHASE_COMMAND);
    let command = purchase_command().replace("\"world\":\"main\"", "\"world\":\"isekai\"");
    succeeds(pool, &other(&(command + &graph + &purchase_position() + &purchase_listing()))).await;
    succeeds(pool, &other(&season_end_sql())).await;
}

/// 正例：isekai 已付成交在季末终态后，`refund_season`（证据 ID 是该合同的 position）经 6B 写端全额退款；
/// 证据查询在通过路径上执行到，退款、封闭事实、处置记录与订单阶段事件同事务。
/// 注入（查询恒假）本用例失败。
#[tokio::test]
async fn postgres_consignment_order_resolve_refund_season_accepts_its_own_season_end() {
    let (database, pool) = isekai_paid_sale().await;
    succeeds(&pool, &season_end_sql()).await;
    let version = current_version(&pool).await;
    let events = order_events(&pool).await;
    assert_eq!(funds(&pool).await, (0, 96, 5), "付款已认证，卖款与手续费在账");
    let service = service(database.store().await.expect("运行账号"));

    service
        .resolve(resolve_input(
            1,
            contract(),
            version,
            OrderResolveAction::RefundSeason,
            Uuid::parse_str(POSITION).expect("position"),
        ))
        .await
        .expect("退款");

    assert_eq!(funds(&pool).await, (101, 0, 0));
    assert_eq!((closures(&pool).await, resolutions(&pool).await), (1, 1));
    assert_eq!(order_events(&pool).await, events + 1, "阶段事件与退款同事务追加");
    assert_eq!(ledger_of(&pool, command(1)).await, Some((200, "ok".to_owned())));
}

/// 反例：`refund_season` 用属于别的合同的真实季末终态（第二个 isekai 件的 position）：确定拒绝
/// `evidence_unproven`，零资金痕迹。去掉证据查询里的合同绑定，别的合同的季末事件就能给本合同退款。
#[tokio::test]
async fn postgres_consignment_order_resolve_refuses_a_season_end_of_another_contract() {
    let (database, pool) = isekai_paid_sale().await;
    succeeds(&pool, &season_end_sql()).await;
    seed_other_season_ended(&pool).await;
    let version = current_version(&pool).await;
    let events = order_events(&pool).await;
    let service = service(database.store().await.expect("运行账号"));

    let error = service
        .resolve(resolve_input(
            1,
            contract(),
            version,
            OrderResolveAction::RefundSeason,
            Uuid::parse_str(&other(POSITION)).expect("position"),
        ))
        .await
        .expect_err("别的合同的季末事件不能退款");

    assert_eq!(rejection(&error), Some(DepositBusinessRejection::EvidenceUnproven));
    assert_eq!(funds(&pool).await, (0, 96, 5), "没有任何资金变化");
    assert_eq!((closures(&pool).await, resolutions(&pool).await), (0, 0));
    assert_eq!(order_events(&pool).await, events);
}

/// 反例：买家已确领原件（付款后原件已交付）时再退款就是双付——证据互相矛盾，确定拒绝 `evidence_conflict`，
/// 即使退款证据本身成立；资金、封闭事实、处置与订单阶段事件全部不变。
#[tokio::test]
async fn postgres_consignment_order_resolve_refuses_a_refund_after_the_buyer_claimed_the_original()
{
    let (_database, pool, runtime) = paid_sale_with_loss_proof().await;
    succeeds(&pool, &buyer_claimed_original_sql()).await;
    let service = service(runtime);

    let error = service
        .resolve(resolve_input(
            1,
            contract(),
            1,
            OrderResolveAction::RefundLost,
            Uuid::parse_str(PROOF).expect("证据"),
        ))
        .await
        .expect_err("原件已确领");

    assert_eq!(rejection(&error), Some(DepositBusinessRejection::EvidenceConflict));
    assert_eq!(ledger_of(&pool, command(1)).await, Some((409, "evidence_conflict".to_owned())));
    assert_eq!(funds(&pool).await, (0, 96, 5), "零资金变化");
    assert_eq!((closures(&pool).await, resolutions(&pool).await), (0, 0));
    assert_eq!(order_events(&pool).await, 0);
}

/// 反例（成对证据）：原件已被精确 Take 消费（`refund_returned` 的证据本身成立），之后又见同一原件的确领：
/// 原物领取与原物 RTS 互斥，证据互相矛盾，确定拒绝 `evidence_conflict`；资金、封闭事实与处置全部不变。
/// 正例见 `postgres_consignment_order_resolve_refund_returned_accepts_its_own_consumption`。
#[tokio::test]
async fn postgres_consignment_order_resolve_refund_returned_refuses_when_the_original_was_also_claimed()
 {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    let store = std::sync::Arc::new(database.store().await.expect("以运行账号连接"));
    let rts = Rts { n: 1 };
    let (receipt, claim) = seed_original_started(pool, &store, rts).await;
    return_service(&store)
        .consume(consume_input(&receipt, rts, claim, SIGNED_VALIDATOR, 0xa7))
        .await
        .expect("消费");
    fund_the_sale(&store).await;
    succeeds(pool, &buyer_claimed_original_from("committed", 6)).await;
    let consumption: Uuid =
        sqlx::query_scalar("SELECT id FROM public.consignment_return_consumptions")
            .fetch_one(pool)
            .await
            .expect("消费");
    let service = service(database.store().await.expect("运行账号"));
    let events = order_events(pool).await;

    let version = current_version(pool).await;
    let error = service
        .resolve(resolve_input(
            1,
            contract(),
            version,
            OrderResolveAction::RefundReturned,
            consumption,
        ))
        .await
        .expect_err("确领与退件证据矛盾");

    assert_eq!(rejection(&error), Some(DepositBusinessRejection::EvidenceConflict));
    assert_eq!(funds(pool).await, (0, 96, 5), "零资金变化");
    assert_eq!((closures(pool).await, resolutions(pool).await), (0, 0));
    assert_eq!(order_events(pool).await, events);
}
