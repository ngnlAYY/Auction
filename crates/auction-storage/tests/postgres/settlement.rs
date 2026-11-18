//! 结算端到端（领取 → 结算 → 重放 → 完成），全程以运行账号执行。
//!
//! 夹具只用 SQL 写入场次、寄售来源、装备、lot 与用户；被测路径全部走生产端口：
//! `DatabaseCompetitionCommands::place_bid`、`DatabaseAuctionLifecycleCommands`（FinalPhase）、
//! `SettlementWorkStore::claim_due/complete` 与 `DatabaseSettlementCommands::settle`，
//! 与 Public/Admin/Worker 使用的是同一批事务 owner。
//!
//! 覆盖：
//! - 竞价事实与结算事务都能以 `auction_runtime` 成员身份提交（无 DDL、无 owner 权限）；
//! - 场次进入 settled，有出价的 lot 成交给最高出价者，无出价的 lot 流拍；
//! - 持久化义务与返回计划逐项一致：成交 lot 有买方 COD、卖方货款与或有退款，流拍 lot 有退回；
//! - 不变量 1：同命令同摘要重放返回同一计划且不重复写入；同命令 ID 不同摘要冲突；
//! - 结算金额守恒：成交价 = 买方应付 + 补偿 − 税费 − 卖方所得 的逆向核对由领域校验保证，
//!   这里核对持久化行与计划一致；
//! - 进入 Final、领取与结算的实时事件能经 Public 回放读出，领取与结算事件沿用业务发生时间。
//!
//! 同一夹具继续驱动结算之后的三条路径（起点都是真实结算写入的事实，而不是手写状态）：
//! - 恢复推进：付款期限已过但付款邮件未证实寄出时只延后并审计，不创建提醒；
//! - 人工 Sent 证明：不满足准入的投递只返回阻断项，确认失败关闭且不追加事实。
//!   准入成功分支需要完整的发送尝试、凭据与资金事实，尚无夹具，留作后续覆盖；
//! - 装备损失：运行账号按资金锁序取得 generation、world、资金账户与完整财务责任集合后，
//!   不满足准入的命令失败关闭且不追加事实。
//!
//! 仅在 `test-db` feature 下编译，入口见 `mise run test-db`。
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::BTreeSet;
use std::sync::Arc;

use auction_application::auction::{
    AuctionLifecycleAction, AuctionLifecycleActor, AuctionLifecycleActorType,
    AuctionLifecycleCommand, AuctionLifecycleCommandPort, AuctionLifecyclePolicy,
    AuctionPolicyTemplate, CompetitionCommandPort, CreateProxyCommand, PlaceBidCommand,
    PublicAccountPageRequest, PublicAccountReader, PublicBidLogListRequest, PublicBidLogReader,
    PublicMyBidsRequest,
};
use auction_application::core::{ApplicationError, RequestHash};
use auction_application::delivery::{
    DeliverySentConfirmationPort, ManualSentEvidence, SentConfirmationCommand,
};
use auction_application::public::{PublicCompensationLotRequest, PublicCompensationReader};
use auction_application::realtime::{PublicRealtimeReader, PublicRealtimeRequest};
use auction_application::recovery::RecoveryWorkStore;
use auction_application::settlement::{
    EquipmentLossCommand, SettleAuctionCommand, SettlementCommandPort, SettlementEquipmentLossPort,
    SettlementReportReader, SettlementWorkClaim, SettlementWorkStore,
    settlement_complete_request_hash,
};
use auction_domain::{
    AuctionId, BidAmount, CommandId, LotId, ProxyMode, SettlementObligationKind,
    SettlementObligationState, SettlementPlan, UserId,
};
use auction_storage::{
    DatabaseAuctionLifecycleCommands, DatabaseCompetitionCommands, DatabaseSettlementCommands,
    DatabaseStore,
};
use auction_time::{Duration, Timestamp};
use uuid::Uuid;

use crate::support;
use crate::support::fixtures::{policy_snapshot, seed_user, stable_id};

const SEED: &str = "settlement-e2e";
const START_PRICE: i64 = 50_000;
/// 0 ppm 时冻结策略的统一最低加价（`POLICY_MINIMUM_INCREMENT`）。
const BID_STEP: i64 = 30_000;
const SELLER: &str = "settle-seller";

struct Seeded {
    auction_id: Uuid,
    sold_lot: Uuid,
    unsold_lot: Uuid,
    bidders: [Uuid; 2],
    deadline: Timestamp,
}

/// 写入一场 live 场次与两件带寄售来源的 lot；只写夹具，不经过被测端口。
async fn seed(admin: &DatabaseStore) -> Seeded {
    let pool = admin.postgres_pool();
    let deadline = Timestamp::now() + Duration::hours(2);
    let opened = deadline - Duration::hours(4);
    let auction_id = stable_id(SEED, "auction");
    sqlx::query(
        "INSERT INTO public.auctions
            (id, world_id, sequence, title, status, scheduled_open_at, preview_scheduled_at,
             scheduled_end_at, effective_end_at, preview_opened_at, opened_at, policy_snapshot)
         VALUES ($1, 'main', 1, 'settlement e2e', 'live', $2, $2, $3, $3, $2, $2, $4)",
    )
    .bind(auction_id)
    .bind(opened)
    .bind(deadline)
    .bind(policy_snapshot(deadline, START_PRICE, BID_STEP))
    .execute(pool)
    .await
    .expect("写入测试场次");
    sqlx::query("INSERT INTO public.realtime_streams (stream_key, next_sequence) VALUES ($1, 1)")
        .bind(format!("auction:{auction_id}"))
        .execute(pool)
        .await
        .expect("写入测试 stream");

    let bidders = [stable_id(SEED, "bidder-0"), stable_id(SEED, "bidder-1")];
    for (index, bidder) in bidders.iter().enumerate() {
        seed_user(pool, *bidder, &format!("{SEED}-bidder-{index}")).await;
    }

    let mut lots = Vec::with_capacity(2);
    for index in 0..2_i32 {
        let equipment_id = stable_id(SEED, &format!("equipment-{index}"));
        sqlx::query(
            "INSERT INTO public.equipment
                (id, world_id, external_equipment_id, name, category, attributes, collected_at)
             VALUES ($1, 'main', $2, $3, 'weapon', '{}'::jsonb, $4)",
        )
        .bind(equipment_id)
        // 游戏装备编号是纯数字；公开逐件结算按它投影装备链接并校验格式。
        .bind(format!("{}", 9_100_000 + index))
        .bind(format!("{SEED} equipment {index}"))
        .bind(opened)
        .execute(pool)
        .await
        .expect("写入测试装备");

        // 结算需要卖方身份：每件 lot 挂一条 Admin 来源的寄售记录。
        let submission_id = stable_id(SEED, &format!("submission-{index}"));
        let command_id = stable_id(SEED, &format!("submission-command-{index}"));
        let mut fingerprint = [0_u8; 32];
        fingerprint[..16].copy_from_slice(submission_id.as_bytes());
        fingerprint[16..].copy_from_slice(command_id.as_bytes());
        sqlx::query(
            "INSERT INTO public.auction_commands
                (command_id, command_kind, aggregate_id, request_hash, result_code)
             VALUES ($1, 'admin_intake_item', $2, $3, 'accepted')",
        )
        .bind(command_id)
        .bind(auction_id)
        .bind(fingerprint.as_slice())
        .execute(pool)
        .await
        .expect("写入寄售来源命令");
        sqlx::query(
            "INSERT INTO public.auction_submissions
                (id, source_command_id, auction_id, seller, received_at,
                 source_kind, source_reference, source_fingerprint)
             VALUES ($1, $2, $3, $4, $5, 'admin', $6, $7)",
        )
        .bind(submission_id)
        .bind(command_id)
        .bind(auction_id)
        .bind(SELLER)
        .bind(opened)
        .bind(format!("{SEED}-submission-{index}"))
        .bind(fingerprint.as_slice())
        .execute(pool)
        .await
        .expect("写入寄售记录");

        let lot_id = stable_id(SEED, &format!("lot-{index}"));
        sqlx::query(
            "INSERT INTO public.lots
                (id, auction_id, equipment_id, lot_number, status, start_price, current_price,
                 effective_end_at, category_key, category_number, submission_id)
             VALUES ($1, $2, $3, $4, 'live', $5, $5, $6, 'weapon', $4, $7)",
        )
        .bind(lot_id)
        .bind(auction_id)
        .bind(equipment_id)
        .bind(index + 1)
        .bind(START_PRICE)
        .bind(deadline)
        .bind(submission_id)
        .execute(pool)
        .await
        .expect("写入测试 lot");
        lots.push(lot_id);
    }

    Seeded { auction_id, sold_lot: lots[0], unsold_lot: lots[1], bidders, deadline }
}

fn bid(round: usize, lot_id: Uuid, bidder_id: Uuid, amount: i64) -> PlaceBidCommand {
    let lot_id = LotId::from_uuid(lot_id);
    let bidder_id = UserId::from_uuid(bidder_id);
    let amount = BidAmount::new(amount).expect("合法竞价金额");
    PlaceBidCommand {
        command_id: CommandId::from_uuid(stable_id(SEED, &format!("bid-{round}"))),
        lot_id,
        bidder_id,
        amount,
        request_hash: PlaceBidCommand::legacy_request_hash(lot_id, bidder_id, amount)
            .expect("稳定请求摘要"),
        request_id: Some(format!("{SEED}-bid-{round}")),
    }
}

fn final_phase(auction_id: Uuid) -> AuctionLifecycleCommand {
    AuctionLifecycleCommand {
        command_id: CommandId::from_uuid(stable_id(SEED, "final-phase")),
        auction_id: AuctionId::from_uuid(auction_id),
        action: AuctionLifecycleAction::FinalPhase,
        expected_state_version: None,
        reason: None,
        request_hash: RequestHash::from_canonical_bytes(format!("{SEED}-final-phase").as_bytes()),
        request_id: Some(format!("{SEED}-final-phase")),
        actor: AuctionLifecycleActor {
            actor_type: AuctionLifecycleActorType::Admin,
            actor_id: "settlement-test-admin".to_owned(),
        },
    }
}

async fn obligation_rows(pool: &sqlx::PgPool, auction_id: Uuid) -> BTreeSet<(Uuid, Uuid, String)> {
    sqlx::query_as::<_, (Uuid, Uuid, String)>(
        "SELECT id, lot_id, obligation_kind FROM public.settlement_obligations
         WHERE auction_id = $1",
    )
    .bind(auction_id)
    .fetch_all(pool)
    .await
    .expect("读取结算义务")
    .into_iter()
    .collect()
}

fn kind_label(kind: SettlementObligationKind) -> &'static str {
    match kind {
        SettlementObligationKind::WinnerEquipmentCod => "winner_equipment_cod",
        SettlementObligationKind::WinnerPaymentCarrierCod => "winner_payment_carrier_cod",
        SettlementObligationKind::WinnerEquipmentHandoff => "winner_equipment_handoff",
        SettlementObligationKind::SellerPayout => "seller_payout",
        SettlementObligationKind::BuyerRefund => "buyer_refund",
        SettlementObligationKind::UnsoldReturn => "unsold_return",
        SettlementObligationKind::WithdrawnReturn => "withdrawn_return",
    }
}

struct Settled {
    claim: SettlementWorkClaim,
    command: SettleAuctionCommand,
    plan: SettlementPlan,
    settlements: DatabaseSettlementCommands,
    now: Timestamp,
}

/// 经生产端口完成出价、进入 Final、领取与结算，全部以运行账号提交；返回尚未完成 fence 的领取。
async fn settle_through_ports(
    admin: &DatabaseStore,
    service: &DatabaseStore,
    seeded: &Seeded,
) -> Settled {
    let pool = admin.postgres_pool();
    // 竞价与生命周期走生产端口，并以运行账号提交。
    let bids = DatabaseCompetitionCommands::new(Arc::new(service.clone()));
    for (round, (bidder, amount)) in [
        (seeded.bidders[0], START_PRICE + BID_STEP),
        (seeded.bidders[1], START_PRICE + 2 * BID_STEP),
    ]
    .into_iter()
    .enumerate()
    {
        bids.place_bid(bid(round, seeded.sold_lot, bidder, amount))
            .await
            .unwrap_or_else(|error| panic!("第 {round} 笔出价必须被接受：{error}"));
    }
    let lifecycle = DatabaseAuctionLifecycleCommands::new(
        Arc::new(service.clone()),
        AuctionLifecyclePolicy::new(true, true),
        AuctionPolicyTemplate::default(),
    );
    lifecycle.execute(final_phase(seeded.auction_id)).await.expect("进入最终阶段");

    // 以截止之后的时刻领取；due_at 冻结为最晚 lot 有效截止。
    let now = seeded.deadline + Duration::seconds(1);
    let mut claims = service
        .claim_due("settlement-test-worker".to_owned(), now, now + Duration::minutes(5), 10)
        .await
        .expect("领取到期结算");
    assert_eq!(claims.len(), 1, "只有本场次到期");
    let claim = claims.pop().expect("结算领取");
    assert_eq!(claim.auction_id.as_uuid(), seeded.auction_id);
    // 进入 Final 会重设 lot 有效截止；领取冻结的世代必须等于锁内最晚的 lot 截止。
    let latest_end: Timestamp = sqlx::query_scalar(
        "SELECT max(effective_end_at) FROM public.lots WHERE auction_id = $1 AND status = 'live'",
    )
    .bind(seeded.auction_id)
    .fetch_one(pool)
    .await
    .expect("读取最晚 lot 截止");
    assert_eq!(claim.due_at, latest_end);
    assert!(!claim.completion_only);

    let command = SettleAuctionCommand {
        command_id: claim.settlement_command_id,
        workflow_id: claim.workflow_id,
        claim_command_id: claim.claim_command_id,
        auction_id: claim.auction_id,
        due_at: claim.due_at,
        now: claim.due_at,
        request_hash: settlement_complete_request_hash(
            seeded.auction_id,
            claim.workflow_id,
            claim.due_at,
        )
        .expect("结算命令摘要"),
    };
    let settlements = DatabaseSettlementCommands::new(Arc::new(service.clone()));
    let plan = settlements.settle(command).await.expect("结算必须以运行账号提交");
    Settled { claim, command, plan, settlements, now }
}

#[tokio::test]
async fn postgres_settlement_claims_settles_replays_and_completes_as_runtime_account() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let admin = database.maintenance_store().await.expect("以维护账号连接");
    let service = database.store().await.expect("以运行账号连接");
    let seeded = seed(&admin).await;
    let pool = admin.postgres_pool();
    let Settled { claim, command, plan, settlements, now } =
        settle_through_ports(&admin, &service, &seeded).await;

    // 场次与 lot 终态。
    let status: String = sqlx::query_scalar("SELECT status FROM public.auctions WHERE id = $1")
        .bind(seeded.auction_id)
        .fetch_one(pool)
        .await
        .expect("读取场次状态");
    assert_eq!(status, "settled");
    let (sold_status, leader): (String, Option<Uuid>) =
        sqlx::query_as("SELECT status, leader_user_id FROM public.lots WHERE id = $1")
            .bind(seeded.sold_lot)
            .fetch_one(pool)
            .await
            .expect("读取成交 lot");
    assert_eq!(sold_status, "sold");
    assert_eq!(leader, Some(seeded.bidders[1]), "最高出价者成交");
    let unsold_status: String = sqlx::query_scalar("SELECT status FROM public.lots WHERE id = $1")
        .bind(seeded.unsold_lot)
        .fetch_one(pool)
        .await
        .expect("读取流拍 lot");
    assert_eq!(unsold_status, "unsold");

    // 成交价与结算明细一致。
    let end_price: i64 =
        sqlx::query_scalar("SELECT end_price FROM public.lot_settlements WHERE lot_id = $1")
            .bind(seeded.sold_lot)
            .fetch_one(pool)
            .await
            .expect("读取成交明细");
    assert_eq!(end_price, START_PRICE + 2 * BID_STEP);
    assert_eq!(plan.lines.len(), 2, "两件 lot 都有结算明细");

    // 持久化义务与返回计划逐项一致。
    let expected: BTreeSet<(Uuid, Uuid, String)> = plan
        .obligations
        .iter()
        .map(|obligation| {
            (
                obligation.id.as_uuid(),
                obligation.lot_id.as_uuid(),
                kind_label(obligation.kind).to_owned(),
            )
        })
        .collect();
    let persisted = obligation_rows(pool, seeded.auction_id).await;
    assert_eq!(persisted, expected, "持久化义务必须与结算计划一致");
    let kinds_for = |lot: Uuid| -> BTreeSet<&str> {
        persisted
            .iter()
            .filter(|(_, lot_id, _)| *lot_id == lot)
            .map(|(_, _, kind)| kind.as_str())
            .collect()
    };
    assert_eq!(
        kinds_for(seeded.sold_lot),
        BTreeSet::from(["buyer_refund", "seller_payout", "winner_equipment_cod"])
    );
    assert_eq!(kinds_for(seeded.unsold_lot), BTreeSet::from(["unsold_return"]));
    // 买方退款只是预先登记的或有义务，结算时不得立即可执行。
    let refund = plan
        .obligations
        .iter()
        .find(|obligation| obligation.kind == SettlementObligationKind::BuyerRefund)
        .expect("成交 lot 的或有退款");
    assert_eq!(refund.state, SettlementObligationState::Contingent);

    // 不变量 3：生命周期、领取与结算事件与各自事实同事务写入；回放序号连续、负载通过公开
    // 注册表。领取事件沿用冻结的 due_at，结算事件沿用 settled_at。
    let page = service
        .list_public_events(PublicRealtimeRequest {
            auction_id: seeded.auction_id,
            after: 0,
            page_size: 20,
        })
        .await
        .expect("读取场次实时事件")
        .expect("场次可见");
    assert!(!page.refetch_required, "事件序号连续且负载符合注册表");
    let lifecycle: Vec<(&str, i32)> = page
        .events
        .iter()
        .filter(|event| event.kind != "lot.bid_accepted")
        .map(|event| (event.kind.as_str(), event.schema_version))
        .collect();
    assert_eq!(
        lifecycle,
        [("auction.final_phase_entered", 1), ("auction.settling", 1), ("auction.settled", 2)]
    );
    let settled_at: Timestamp =
        sqlx::query_scalar("SELECT settled_at FROM public.auctions WHERE id = $1")
            .bind(seeded.auction_id)
            .fetch_one(pool)
            .await
            .expect("读取结算时间");
    let created_at = |kind: &str| {
        page.events.iter().find(|event| event.kind == kind).map(|event| event.created_at)
    };
    assert_eq!(created_at("auction.settling"), Some(claim.due_at));
    assert_eq!(created_at("auction.settled"), Some(settled_at));

    // 不变量 1：租约内重试只重放同一命令，返回同一计划且不重复写入。
    let replayed = settlements.settle(command).await.expect("同命令重放");
    assert_eq!(replayed, plan);
    assert_eq!(obligation_rows(pool, seeded.auction_id).await, persisted);

    // 同命令 ID、不同摘要必须冲突，不得产生第二份结算。
    let mut conflicting = command;
    conflicting.request_hash = RequestHash::from_canonical_bytes(b"settlement-e2e-conflict");
    settlements.settle(conflicting).await.expect_err("同 ID 不同摘要必须冲突");
    assert_eq!(obligation_rows(pool, seeded.auction_id).await, persisted);

    // 完成 fence 后不再被领取。
    service.complete(claim, now).await.expect("完成结算 workflow");
    let again = service
        .claim_due("settlement-test-worker".to_owned(), now, now + Duration::minutes(5), 10)
        .await
        .expect("再次领取");
    assert!(again.is_empty(), "已完成的结算不得再次领取");

    service.postgres_pool().close().await;
    admin.postgres_pool().close().await;
}

/// 场次当前的结算尝试次数与最近失败码。
async fn settlement_attempt_state(pool: &sqlx::PgPool, auction_id: Uuid) -> (i32, Option<String>) {
    sqlx::query_as(
        "SELECT settlement_attempt_count, settlement_last_error_code
         FROM public.auctions WHERE id = $1",
    )
    .bind(auction_id)
    .fetch_one(pool)
    .await
    .expect("读取结算尝试状态")
}

/// 结算失败重试审计的元数据，按发生顺序。
async fn settlement_retry_audits(pool: &sqlx::PgPool, auction_id: Uuid) -> Vec<serde_json::Value> {
    sqlx::query_scalar(
        "SELECT metadata FROM public.audit_records
         WHERE entity_type = 'auction' AND entity_id = $1
           AND action = 'auction.settlement_retry_scheduled'
         ORDER BY occurred_at",
    )
    .bind(auction_id.to_string())
    .fetch_all(pool)
    .await
    .expect("读取结算失败审计")
}

/// 结算失败经生产端口记录（运行账号）：空说明失败关闭，不推进尝试也不追加审计；有效失败按
/// 数据库退避表推进尝试，并把安全说明与失败码同事务写入重试审计（不变量 5、8）。
#[tokio::test]
async fn postgres_settlement_failure_records_safe_detail_in_retry_audit() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let admin = database.maintenance_store().await.expect("以维护账号连接");
    let service = database.store().await.expect("以运行账号连接");
    let seeded = seed(&admin).await;
    let pool = admin.postgres_pool();
    let lifecycle = DatabaseAuctionLifecycleCommands::new(
        Arc::new(service.clone()),
        AuctionLifecyclePolicy::new(true, true),
        AuctionPolicyTemplate::default(),
    );
    lifecycle.execute(final_phase(seeded.auction_id)).await.expect("进入最终阶段");
    let now = seeded.deadline + Duration::seconds(1);
    let claim = service
        .claim_due("settlement-test-worker".to_owned(), now, now + Duration::minutes(5), 10)
        .await
        .expect("领取到期结算")
        .pop()
        .expect("结算领取");
    let (initial_attempts, _) = settlement_attempt_state(pool, seeded.auction_id).await;

    service
        .fail(claim.clone(), "internal".to_owned(), String::new())
        .await
        .expect_err("空失败说明必须失败关闭");
    assert_eq!(
        settlement_attempt_state(pool, seeded.auction_id).await,
        (initial_attempts, None),
        "拒绝的失败记录不得推进尝试"
    );
    assert!(settlement_retry_audits(pool, seeded.auction_id).await.is_empty());

    let detail = "结算池余额与结算计划不一致";
    service
        .fail(claim.clone(), "settlement_mismatch".to_owned(), detail.to_owned())
        .await
        .expect("记录结算失败");
    assert_eq!(
        settlement_attempt_state(pool, seeded.auction_id).await,
        (initial_attempts + 1, Some("settlement_mismatch".to_owned())),
        "失败记录推进一次尝试并保存失败码"
    );
    let audits = settlement_retry_audits(pool, seeded.auction_id).await;
    assert_eq!(audits.len(), 1, "每次失败只追加一条重试审计");
    assert_eq!(audits[0]["error_code"], "settlement_mismatch");
    assert_eq!(audits[0]["safe_detail"], detail);
    assert_eq!(audits[0]["attempt_count"], initial_attempts + 1);
    assert_eq!(audits[0]["automatic_retry"], true);

    service.postgres_pool().close().await;
    admin.postgres_pool().close().await;
}

/// 结算窗口（领取后、结算提交前）内，本人出价及总数与同一响应里的代理按同一范围照常显示；
/// 竞价日志仍不对 settling 场次开放；世界停用后 settling 场次的出价与代理一并隐藏。
#[tokio::test]
async fn postgres_my_bids_keep_bids_and_proxies_visible_while_settling() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let admin = database.maintenance_store().await.expect("以维护账号连接");
    let service = database.store().await.expect("以运行账号连接");
    let seeded = seed(&admin).await;
    let pool = admin.postgres_pool();
    let commands = DatabaseCompetitionCommands::new(Arc::new(service.clone()));
    commands
        .place_bid(bid(0, seeded.sold_lot, seeded.bidders[0], START_PRICE + BID_STEP))
        .await
        .expect("普通出价必须被接受");
    let lot_id = LotId::from_uuid(seeded.sold_lot);
    let proxy_owner = UserId::from_uuid(seeded.bidders[1]);
    let maximum = BidAmount::new(START_PRICE + 4 * BID_STEP).expect("合法代理上限");
    commands
        .create_proxy(CreateProxyCommand {
            command_id: CommandId::from_uuid(stable_id(SEED, "proxy-0")),
            lot_id,
            bidder_id: proxy_owner,
            maximum_amount: maximum,
            mode: ProxyMode::Fast,
            request_hash: CreateProxyCommand::legacy_request_hash(
                lot_id,
                proxy_owner,
                ProxyMode::Fast,
                maximum,
            )
            .expect("稳定请求摘要"),
            request_id: Some(format!("{SEED}-proxy-0")),
        })
        .await
        .expect("快速代理必须被接受");
    let lifecycle = DatabaseAuctionLifecycleCommands::new(
        Arc::new(service.clone()),
        AuctionLifecyclePolicy::new(true, true),
        AuctionPolicyTemplate::default(),
    );
    lifecycle.execute(final_phase(seeded.auction_id)).await.expect("进入最终阶段");
    let now = seeded.deadline + Duration::seconds(1);
    let claims = service
        .claim_due("settlement-test-worker".to_owned(), now, now + Duration::minutes(5), 10)
        .await
        .expect("领取到期结算");
    assert_eq!(claims.len(), 1, "只有本场次到期");
    let status: String = sqlx::query_scalar("SELECT status FROM public.auctions WHERE id = $1")
        .bind(seeded.auction_id)
        .fetch_one(pool)
        .await
        .expect("读取场次状态");
    assert_eq!(status, "settling");

    let request = PublicMyBidsRequest {
        bids: PublicAccountPageRequest { limit: 20, offset: 0 },
        proxies: PublicAccountPageRequest { limit: 20, offset: 0 },
    };
    let owner_page =
        service.list_public_user_bids(seeded.bidders[1], request).await.expect("读取代理持有者");
    assert_eq!((owner_page.bids.len(), owner_page.bid_total), (1, 1), "代理出价事实照常显示");
    assert_eq!((owner_page.proxies.len(), owner_page.proxy_total), (1, 1), "代理照常显示");
    assert_eq!(owner_page.bids[0].lot_id, seeded.sold_lot);
    let bidder_page =
        service.list_public_user_bids(seeded.bidders[0], request).await.expect("读取普通出价者");
    assert_eq!((bidder_page.bids.len(), bidder_page.bid_total), (1, 1), "普通出价照常显示");
    assert_eq!((bidder_page.proxies.len(), bidder_page.proxy_total), (0, 0));

    let bid_log = service
        .list_public_bid_log(PublicBidLogListRequest {
            auction_id: seeded.auction_id,
            world_id: None,
            lot_id: None,
            lot_code: None,
            limit: 20,
            offset: 0,
            cursor: None,
        })
        .await
        .expect("读取竞价日志");
    assert!(bid_log.is_none(), "竞价日志不对 settling 场次开放");

    sqlx::query("UPDATE public.worlds SET enabled = false WHERE id = 'main'")
        .execute(pool)
        .await
        .expect("停用世界");
    let hidden =
        service.list_public_user_bids(seeded.bidders[1], request).await.expect("读取停用世界");
    assert_eq!(
        (hidden.bids.len(), hidden.bid_total, hidden.proxies.len(), hidden.proxy_total),
        (0, 0, 0, 0),
        "世界停用后未结算场次的出价与代理一并隐藏"
    );

    service.postgres_pool().close().await;
    admin.postgres_pool().close().await;
}

/// 结算报告在一个可重复读只读快照内读取（运行账号）：结算前没有摘要与明细，不存在的场次
/// 返回 None；结算后摘要、账本笔数与 lot 明细都与持久化事实一致，累计领先时长在报告与公开
/// 逐件结算中按同一公式得出相同值。
#[tokio::test]
async fn postgres_settlement_report_reads_consistent_snapshot_as_runtime_account() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let admin = database.maintenance_store().await.expect("以维护账号连接");
    let service = database.store().await.expect("以运行账号连接");
    let seeded = seed(&admin).await;
    let pool = admin.postgres_pool();
    let auction_id = AuctionId::from_uuid(seeded.auction_id);

    let before =
        service.settlement_report(auction_id).await.expect("读取未结算报告").expect("场次存在");
    assert!(before.settlement.is_none(), "未结算场次没有结算摘要");
    assert!(before.lots.is_empty(), "未结算场次不返回 lot 明细");
    let missing = service
        .settlement_report(AuctionId::from_uuid(stable_id(SEED, "missing-auction")))
        .await
        .expect("读取不存在的场次");
    assert!(missing.is_none());

    settle_through_ports(&admin, &service, &seeded).await;
    let report =
        service.settlement_report(auction_id).await.expect("读取结算报告").expect("场次存在");
    assert_eq!(report.auction.status, "settled");
    let summary = report.settlement.expect("已结算场次必须有结算摘要");
    let (tax_total, compensation_total, closing_balance, movements): (i64, i64, i64, i64) =
        sqlx::query_as(
            "SELECT settlement.tax_total, settlement.compensation_total,
                    settlement.closing_pool_balance,
                    (SELECT count(*) FROM public.compensation_ledger AS ledger
                     WHERE ledger.auction_id = settlement.auction_id)
             FROM public.auction_settlements AS settlement WHERE settlement.auction_id = $1",
        )
        .bind(seeded.auction_id)
        .fetch_one(pool)
        .await
        .expect("读取结算摘要事实");
    assert_eq!(summary.tax_income, tax_total);
    assert_eq!(summary.compensation_out, compensation_total);
    assert_eq!(summary.closing_balance, closing_balance);
    assert_eq!(summary.movement_count, movements);

    assert_eq!(report.lots.len(), 2, "两件 lot 都有结算明细");
    let sold = report
        .lots
        .iter()
        .find(|lot| lot.id.as_uuid() == seeded.sold_lot)
        .expect("报告包含成交 lot");
    assert_eq!(sold.status, "sold");
    assert_eq!(sold.final_price, START_PRICE + 2 * BID_STEP);
    assert_eq!(sold.winner.as_deref(), Some("settlement-e2e-bidder-1"));
    assert_eq!(sold.seller.as_deref(), Some(SELLER));
    let (buyer_amount, seller_amount, tax_amount): (i64, i64, i64) = sqlx::query_as(
        "SELECT buyer_amount, seller_amount, tax_amount FROM public.lot_settlements
         WHERE lot_id = $1",
    )
    .bind(seeded.sold_lot)
    .fetch_one(pool)
    .await
    .expect("读取成交 lot 结算事实");
    assert_eq!(
        (sold.buyer_amount, sold.seller_amount, sold.tax_amount),
        (buyer_amount, seller_amount, tax_amount)
    );
    // 获胜者只有最后一笔出价，持有到 lot 有效截止；报告与公开逐件结算都按结算锁内的同一
    // 公式重放，结果必须与此一致。
    let (held_from, held_until): (Timestamp, Timestamp) = sqlx::query_as(
        "SELECT bid.created_at, lot.effective_end_at
         FROM public.bids AS bid
         JOIN public.lots AS lot ON lot.id = bid.lot_id
         WHERE bid.lot_id = $1 AND bid.bidder_user_id = lot.leader_user_id",
    )
    .bind(seeded.sold_lot)
    .fetch_one(pool)
    .await
    .expect("读取获胜出价时间与 lot 有效截止");
    let expected_held = (held_until - held_from).num_nanoseconds().expect("持有时长不溢出");
    assert!(expected_held > 0, "获胜出价必须早于 lot 有效截止");
    assert_eq!(sold.leader_held_nanos, expected_held);
    let public_lots = service
        .list_public_compensation_lots(
            seeded.auction_id,
            PublicCompensationLotRequest { limit: 10, offset: 0 },
        )
        .await
        .expect("读取公开逐件结算")
        .expect("已结算场次有公开逐件结算");
    let public_sold = public_lots
        .entries
        .iter()
        .find(|entry| entry.lot_id == seeded.sold_lot)
        .expect("公开逐件结算包含成交 lot");
    assert_eq!(public_sold.leader_held_nanos, expected_held);
    assert_eq!(public_sold.hold_ppm, sold.hold_ppm);
    let unsold = report
        .lots
        .iter()
        .find(|lot| lot.id.as_uuid() == seeded.unsold_lot)
        .expect("报告包含流拍 lot");
    assert_eq!(unsold.status, "unsold");
    assert_eq!(unsold.winner, None);
    assert_eq!(unsold.leader_held_nanos, 0);

    service.postgres_pool().close().await;
    admin.postgres_pool().close().await;
}

async fn recovery_row(
    pool: &sqlx::PgPool,
    auction_id: Uuid,
) -> (Uuid, String, i64, i32, Timestamp, Option<Timestamp>) {
    sqlx::query_as(
        "SELECT recovery.id, recovery.stage, recovery.state_version, recovery.reminder_count,
                recovery.payment_deadline_at, recovery.next_attempt_at
         FROM public.settlement_recoveries AS recovery
         JOIN public.settlement_obligations AS obligation
           ON obligation.id = recovery.payment_obligation_id
         WHERE obligation.auction_id = $1",
    )
    .bind(auction_id)
    .fetch_one(pool)
    .await
    .expect("读取结算建立的恢复记录")
}

async fn deferred_audits(pool: &sqlx::PgPool, recovery_id: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM public.audit_records
         WHERE action = 'recovery.payment_reminder_deferred' AND entity_id = $1
           AND metadata->>'reason' = 'payment_mail_not_sent'",
    )
    .bind(recovery_id.to_string())
    .fetch_one(pool)
    .await
    .expect("读取延后审计")
}

/// 付款期限已过但付款邮件没有发送证明时，恢复推进只延后并审计，不创建逾期提醒、
/// 不改冻结的付款期限；延后窗口内再次推进是空操作。起点是真实结算建立的恢复记录。
#[tokio::test]
async fn postgres_recovery_defers_overdue_reminder_without_payment_mail_proof_as_runtime_account() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let admin = database.maintenance_store().await.expect("以维护账号连接");
    let service = database.store().await.expect("以运行账号连接");
    let seeded = seed(&admin).await;
    let pool = admin.postgres_pool();
    let Settled { claim, now, .. } = settle_through_ports(&admin, &service, &seeded).await;
    service.complete(claim, now).await.expect("完成结算 workflow");

    let (recovery_id, stage, version, reminders, deadline, next_attempt) =
        recovery_row(pool, seeded.auction_id).await;
    assert_eq!(stage, "awaiting_payment");
    assert_eq!(reminders, 0);
    assert_eq!(next_attempt, None);

    // 期限之前：没有到期工作。
    let early =
        service.advance_due_case(deadline - Duration::minutes(1)).await.expect("期限前推进");
    assert_eq!(early, None, "付款期限之前不得推进");

    // 期限之后、付款邮件未证实寄出：只延后。
    let overdue_at = deadline + Duration::minutes(1);
    let deferred = service
        .advance_due_case(overdue_at)
        .await
        .expect("逾期推进")
        .expect("逾期 case 必须被处理");
    assert_eq!(deferred.recovery_id.as_uuid(), recovery_id);
    assert_eq!(deferred.stage, "awaiting_payment");
    assert_eq!(deferred.reminder_count, 0);
    assert_eq!(deferred.state_version, version, "延后不推进状态版本");
    assert!(!deferred.completed);

    let (_, stage_after, version_after, reminders_after, deadline_after, next_after) =
        recovery_row(pool, seeded.auction_id).await;
    assert_eq!(stage_after, "awaiting_payment");
    assert_eq!(version_after, version);
    assert_eq!(reminders_after, 0);
    assert_eq!(deadline_after, deadline, "不变量 10：冻结的付款期限不得改写");
    assert_eq!(next_after, Some(overdue_at + Duration::minutes(15)));
    let actions: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.settlement_recovery_actions WHERE recovery_id = $1",
    )
    .bind(recovery_id)
    .fetch_one(pool)
    .await
    .expect("读取恢复动作");
    assert_eq!(actions, 0, "没有发送证明时不得创建逾期提醒");
    assert_eq!(deferred_audits(pool, recovery_id).await, 1);

    // 延后窗口内再次推进：空操作，不追加审计。
    let within_backoff =
        service.advance_due_case(overdue_at + Duration::minutes(5)).await.expect("延后窗口内推进");
    assert_eq!(within_backoff, None);
    assert_eq!(deferred_audits(pool, recovery_id).await, 1);

    // 窗口过后仍无证明：继续延后而不是提醒。
    let again = service
        .advance_due_case(overdue_at + Duration::minutes(16))
        .await
        .expect("窗口后推进")
        .expect("窗口后再次处理");
    assert_eq!(again.stage, "awaiting_payment");
    assert_eq!(deferred_audits(pool, recovery_id).await, 2);

    service.postgres_pool().close().await;
    admin.postgres_pool().close().await;
}

fn manual_evidence() -> ManualSentEvidence {
    let sent_minute =
        Timestamp::now().with_second(0).and_then(|value| value.with_nanosecond(0)).expect("整分钟");
    ManualSentEvidence {
        schema_version: 1,
        world_id: "main".to_owned(),
        external_mail_id: "424242".to_owned(),
        sender_uid: "1001".to_owned(),
        sender_name: "AuctionBot".to_owned(),
        recipient: "SettlementBuyer".to_owned(),
        subject: "Auction payment".to_owned(),
        sent_minute,
        observed_at: sent_minute + Duration::seconds(30),
        original_body_digest: [7; 32],
        observed_body_digest: [7; 32],
        remaining_credits: 0,
        cod_pending: 0,
        equipment_ids: Vec::new(),
        collected: false,
        receipt: String::new(),
    }
}

/// 人工 Sent 证明的准入以运行账号读取完整锁定快照并由纯函数判定；不满足准入的投递
/// 只返回阻断项，`confirm_sent` 失败关闭且不追加任何证明事实或改变投递状态。
#[tokio::test]
async fn postgres_manual_sent_confirmation_rejects_inadmissible_delivery_as_runtime_account() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let admin = database.maintenance_store().await.expect("以维护账号连接");
    let service = database.store().await.expect("以运行账号连接");
    let seeded = seed(&admin).await;
    let pool = admin.postgres_pool();
    let Settled { claim, now, .. } = settle_through_ports(&admin, &service, &seeded).await;
    service.complete(claim, now).await.expect("完成结算 workflow");

    // 不存在的投递：只给出阻断项。
    let missing = service
        .preview_sent_confirmation(Uuid::new_v4(), manual_evidence(), None)
        .await
        .expect("不存在的投递预览");
    assert!(!missing.eligible);
    assert_eq!(missing.blockers, vec!["delivery_not_found".to_owned()]);

    // 结算后真实存在、但从未进入发送围栏的投递（被超提醒邮件）：不具备人工 Sent 准入。
    let (delivery_id, status_before, fence_before): (Uuid, String, i64) = sqlx::query_as(
        "SELECT id, status, fencing_version FROM public.deliveries ORDER BY created_at, id LIMIT 1",
    )
    .fetch_one(pool)
    .await
    .expect("读取结算后的投递");
    let preview = service
        .preview_sent_confirmation(delivery_id, manual_evidence(), None)
        .await
        .expect("真实投递预览");
    assert!(!preview.eligible, "未进入发送围栏的投递不得准入：{:?}", preview.blockers);
    // 这是被超提醒邮件（非结算载荷形状）：必须给出范围阻断项，而不是以反序列化错误中止。
    assert!(
        preview.blockers.iter().any(|blocker| blocker == "unsupported_delivery_scope"),
        "{:?}",
        preview.blockers
    );
    assert!(
        preview
            .blockers
            .iter()
            .any(|blocker| blocker == "requires_unclaimed_write_unknown_or_admin_failure"),
        "{:?}",
        preview.blockers
    );

    let command = SentConfirmationCommand {
        command_id: Uuid::new_v4(),
        delivery_id,
        expected_fencing_version: fence_before.max(1),
        actor: "settlement-test-operator".to_owned(),
        operator_identity: None,
        reason: "manual Sent observation".to_owned(),
        evidence: manual_evidence(),
        accept_collected_assets: false,
        accept_attempt_history: false,
    };
    service.confirm_sent(command, None).await.expect_err("不满足准入时必须失败关闭");

    let (status_after, fence_after): (String, i64) =
        sqlx::query_as("SELECT status, fencing_version FROM public.deliveries WHERE id = $1")
            .bind(delivery_id)
            .fetch_one(pool)
            .await
            .expect("读取投递终态");
    assert_eq!((status_after, fence_after), (status_before, fence_before));
    for table in ["delivery_sent_confirmations", "delivery_sent_bindings", "delivery_dispositions"]
    {
        let count: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT count(*) FROM public.{table} WHERE delivery_id = $1"
        )))
        .bind(delivery_id)
        .fetch_one(pool)
        .await
        .expect("读取证明事实");
        assert_eq!(count, 0, "{table} 不得追加");
    }

    service.postgres_pool().close().await;
    admin.postgres_pool().close().await;
}

/// 装备损失命令以运行账号依次取得 generation 行锁、全部 world、只记账资金账户与完整财务责任
/// 集合（与现金对账同一锁序），再读快照判定准入；不满足准入时按依赖不匹配失败关闭（而不是
/// 锁定阶段的数据库失败），且不追加任何损失事实。
#[tokio::test]
async fn postgres_equipment_loss_takes_funding_lock_order_then_rejects_inadmissible_lot() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let admin = database.maintenance_store().await.expect("以维护账号连接");
    let service = database.store().await.expect("以运行账号连接");
    let seeded = seed(&admin).await;
    let pool = admin.postgres_pool();
    let Settled { claim, now, .. } = settle_through_ports(&admin, &service, &seeded).await;
    service.complete(claim, now).await.expect("完成结算 workflow");
    sqlx::query(
        "INSERT INTO public.settlement_funding_accounts (uid, status, state_version, accounting_mode)
         VALUES ('1001', 'ready', 1, 'record_only')",
    )
    .execute(pool)
    .await
    .expect("写入只记账资金账户");

    let command = EquipmentLossCommand {
        command_id: Uuid::new_v4(),
        lot_id: seeded.sold_lot,
        actor: "settlement-test-operator".to_owned(),
        operator_identity: None,
        reason: "equipment lost in transit".to_owned(),
        funding_uid: "1001".to_owned(),
        // 版本故意与快照不符：即使准入成立也必须拒绝，结果只取决于锁定之后的判定。
        expected_recovery_version: 999,
        expected_payment_version: 999,
        expected_delivery_fencing_version: 0,
        expected_seller_amount: 1,
        buyer_compensation_external: true,
    };
    let error =
        service.resolve_equipment_loss(command).await.expect_err("不满足准入时必须失败关闭");
    assert!(
        matches!(error, ApplicationError::FulfillmentDependencyMismatch),
        "锁定阶段不得以数据库失败中止：{error:?}"
    );
    let losses: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.settlement_equipment_losses WHERE lot_id = $1",
    )
    .bind(seeded.sold_lot)
    .fetch_one(pool)
    .await
    .expect("读取损失事实");
    assert_eq!(losses, 0, "失败关闭不得追加损失事实");

    service.postgres_pool().close().await;
    admin.postgres_pool().close().await;
}

/// 本场次各义务种类的或有未激活标记。
async fn dormant_flags(pool: &sqlx::PgPool, auction_id: Uuid) -> BTreeSet<(String, bool)> {
    sqlx::query_as::<_, (String, bool)>(
        "SELECT obligation.obligation_kind, fulfillment.dormant
         FROM public.settlement_obligations AS obligation
         JOIN public.settlement_obligation_fulfillments AS fulfillment
           ON fulfillment.obligation_id = obligation.id
         WHERE obligation.auction_id = $1",
    )
    .bind(auction_id)
    .fetch_all(pool)
    .await
    .expect("读取或有未激活标记")
    .into_iter()
    .collect()
}

/// 本场次成交 lot 的或有退款义务。
async fn contingent_refund(pool: &sqlx::PgPool, lot_id: Uuid) -> Uuid {
    sqlx::query_scalar(
        "SELECT id FROM public.settlement_obligations
         WHERE lot_id = $1 AND obligation_kind = 'buyer_refund' AND business_state = 'contingent'",
    )
    .bind(lot_id)
    .fetch_one(pool)
    .await
    .expect("读取或有退款")
}

/// 生产结算只给或有买方退款的履约打上或有未激活标记（迁移 000110），活动入口与履约水位因此不
/// 读它；标记由数据库兜底：把带标记的履约推出初始 pending 的写入当即失败，义务离开 contingent
/// 而不清标记的事务在提交时失败，两者都不留下任何改动。
#[tokio::test]
async fn postgres_settlement_marks_only_contingent_refund_dormant_and_guards_the_flag() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let admin = database.maintenance_store().await.expect("以维护账号连接");
    let service = database.store().await.expect("以运行账号连接");
    let seeded = seed(&admin).await;
    let Settled { claim, now, .. } = settle_through_ports(&admin, &service, &seeded).await;
    service.complete(claim, now).await.expect("完成结算 workflow");
    let pool = service.postgres_pool();

    assert_eq!(
        dormant_flags(pool, seeded.auction_id).await,
        BTreeSet::from([
            ("buyer_refund".to_owned(), true),
            ("seller_payout".to_owned(), false),
            ("unsold_return".to_owned(), false),
            ("winner_equipment_cod".to_owned(), false),
        ]),
        "只有或有退款带标记"
    );
    crate::alert_entry_equivalence::assert_alert_reads_match_full_scan(pool).await;

    let refund = contingent_refund(pool, seeded.sold_lot).await;
    let error = sqlx::query(
        "UPDATE public.settlement_obligation_fulfillments
         SET execution_state = 'failed', last_error_code = 'guard_probe', completed_at = now()
         WHERE obligation_id = $1",
    )
    .bind(refund)
    .execute(pool)
    .await
    .expect_err("带标记的履约不能直接离开初始 pending");
    assert_eq!(
        error.as_database_error().and_then(sqlx::error::DatabaseError::code).as_deref(),
        Some("23514"),
        "由 CHECK 拒绝：{error}"
    );

    let mut transaction = pool.begin().await.expect("开启激活事务");
    sqlx::query("UPDATE public.settlement_obligations SET business_state = 'ready' WHERE id = $1")
        .bind(refund)
        .execute(&mut *transaction)
        .await
        .expect("约束触发器延迟到提交");
    let error = transaction.commit().await.expect_err("激活而不清标记必须在提交时失败");
    assert_eq!(
        error.as_database_error().and_then(sqlx::error::DatabaseError::code).as_deref(),
        Some("23514"),
        "由延迟约束触发器拒绝：{error}"
    );
    let (state, dormant): (String, bool) = sqlx::query_as(
        "SELECT obligation.business_state, fulfillment.dormant
         FROM public.settlement_obligations AS obligation
         JOIN public.settlement_obligation_fulfillments AS fulfillment
           ON fulfillment.obligation_id = obligation.id
         WHERE obligation.id = $1",
    )
    .bind(refund)
    .fetch_one(pool)
    .await
    .expect("读取或有退款");
    assert_eq!((state.as_str(), dormant), ("contingent", true), "失败关闭不留下改动");

    service.postgres_pool().close().await;
    admin.postgres_pool().close().await;
}

/// 把成交 lot 的主世界 COD 改写为装备损失可准入的形状：原投递只读过库存、尚未开始写入就转人工
/// 核对（`not_observed`），付款履约随之停在人工核对；退款与卖方货款保持结算写入的初始状态。
async fn make_equipment_loss_admissible(pool: &sqlx::PgPool, lot_id: Uuid) {
    // 夹具在截止之后结算，结算时刻比真实时钟晚约两小时；平移到过去，使处置事务以真实时钟写入的
    // 更新时间不早于创建时间。
    for statement in [
        "UPDATE public.settlement_obligations
         SET created_at = created_at - interval '3 hours', updated_at = updated_at - interval '3 hours'
         WHERE lot_id = $1",
        "UPDATE public.settlement_obligation_fulfillments
         SET created_at = created_at - interval '3 hours', updated_at = updated_at - interval '3 hours'
         WHERE obligation_id IN (SELECT id FROM public.settlement_obligations WHERE lot_id = $1)",
        "UPDATE public.settlement_recoveries
         SET created_at = created_at - interval '3 hours', updated_at = updated_at - interval '3 hours',
             payment_deadline_at = payment_deadline_at - interval '3 hours'
         WHERE lot_id = $1",
    ] {
        sqlx::query(statement).bind(lot_id).execute(pool).await.expect("平移结算时刻");
    }
    let (payment, amount, equipment_id): (Uuid, i64, Uuid) = sqlx::query_as(
        "SELECT id, amount, equipment_id FROM public.settlement_obligations
         WHERE lot_id = $1 AND obligation_kind = 'winner_equipment_cod'",
    )
    .bind(lot_id)
    .fetch_one(pool)
    .await
    .expect("读取 COD 义务");
    let delivery = Uuid::new_v4();
    let prepare = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload, status,
             fencing_version, attempt_count, claimed_at, sending_at, completed_at, failure_code,
             result_metadata, created_at, updated_at)
         VALUES ($1, $2, decode(repeat('a7', 32), 'hex'), 'settlement', $3, 'settlement buyer',
                 jsonb_build_object('schema_version', 1, 'operation', 'send', 'world', 'main',
                     'template', 'settlement_winner_equipment_cod', 'credits', 0, 'cod', $4,
                     'variables', jsonb_build_object('reference', $3),
                     'equipment_secret_refs', jsonb_build_array(jsonb_build_object(
                         'kind', 'auction_equipment_key', 'id', $5::text))),
                 'manual_reconcile', 1, 1, now(), now(), now(), 'not_observed',
                 '{\"operation\":\"inventory\",\"stage\":\"read\",\"write_started\":false}'::jsonb,
                 now(), now())",
    )
    .bind(delivery)
    .bind(prepare)
    .bind(payment.to_string())
    .bind(amount)
    .bind(equipment_id)
    .execute(pool)
    .await
    .expect("写入只读过库存的原 COD 投递");
    sqlx::query(
        "UPDATE public.settlement_obligation_fulfillments
         SET execution_state = 'manual_reconcile', delivery_id = $2, prepare_command_id = $3,
             prepare_request_digest = decode(repeat('a7', 32), 'hex'),
             last_error_code = 'not_observed', completed_at = updated_at,
             state_version = state_version + 1
         WHERE obligation_id = $1",
    )
    .bind(payment)
    .bind(delivery)
    .bind(prepare)
    .execute(pool)
    .await
    .expect("付款履约转人工核对");
}

/// 装备损失以线下补偿收口时，把仍是 contingent 的退款履约直接收口为 failed：同事务清除或有未激活
/// 标记，退款回到活动入口，由损失处置事实证明已处置；水位读数与全表读法一致且不计失败。
#[tokio::test]
async fn postgres_equipment_loss_closes_dormant_refund_into_the_active_entry() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let admin = database.maintenance_store().await.expect("以维护账号连接");
    let service = database.store().await.expect("以运行账号连接");
    let seeded = seed(&admin).await;
    let Settled { claim, now, .. } = settle_through_ports(&admin, &service, &seeded).await;
    service.complete(claim, now).await.expect("完成结算 workflow");
    let pool = service.postgres_pool();
    sqlx::query(
        "INSERT INTO public.settlement_funding_accounts (uid, status, state_version, accounting_mode)
         VALUES ('1001', 'ready', 1, 'record_only')",
    )
    .execute(pool)
    .await
    .expect("写入只记账资金账户");
    make_equipment_loss_admissible(pool, seeded.sold_lot).await;
    let refund = contingent_refund(pool, seeded.sold_lot).await;

    let preview = service.preview_equipment_loss(seeded.sold_lot).await.expect("预览装备损失");
    assert!(preview.eligible, "夹具应满足装备损失准入：{:?}", preview.blockers);
    service
        .resolve_equipment_loss(EquipmentLossCommand {
            command_id: Uuid::new_v4(),
            lot_id: seeded.sold_lot,
            actor: "settlement-test-operator".to_owned(),
            operator_identity: None,
            reason: "equipment destroyed before the COD left the inventory".to_owned(),
            funding_uid: "1001".to_owned(),
            expected_recovery_version: preview.expected_recovery_version,
            expected_payment_version: preview.expected_payment_version,
            expected_delivery_fencing_version: preview.expected_delivery_fencing_version,
            expected_seller_amount: preview.seller_amount,
            buyer_compensation_external: true,
        })
        .await
        .expect("装备损失收口带标记的退款");

    let (state, execution, error_code, dormant): (String, String, Option<String>, bool) =
        sqlx::query_as(
            "SELECT obligation.business_state, fulfillment.execution_state,
                    fulfillment.last_error_code, fulfillment.dormant
             FROM public.settlement_obligations AS obligation
             JOIN public.settlement_obligation_fulfillments AS fulfillment
               ON fulfillment.obligation_id = obligation.id
             WHERE obligation.id = $1",
        )
        .bind(refund)
        .fetch_one(pool)
        .await
        .expect("读取退款履约");
    assert_eq!(
        (state.as_str(), execution.as_str(), error_code.as_deref(), dormant),
        ("contingent", "failed", Some("operator_external_buyer_compensation"), false),
        "退款义务仍是 contingent，履约收口为 failed 并清除标记"
    );
    crate::alert_entry_equivalence::assert_alert_reads_match_full_scan(pool).await;

    service.postgres_pool().close().await;
    admin.postgres_pool().close().await;
}
