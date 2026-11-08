//! W6.2：结算端到端（领取 → 结算 → 重放 → 完成），全程以 W9.9 运行账号执行。
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
//!   这里核对持久化行与计划一致。
//!
//! 同一夹具继续驱动结算之后的两条路径（起点都是真实结算写入的事实，而不是手写状态）：
//! - 恢复推进（W14.6/D9）：付款期限已过但付款邮件未证实寄出时只延后并审计，不创建提醒；
//! - 人工 Sent 证明（W3.1）：不满足准入的投递只返回阻断项，确认失败关闭且不追加事实。
//!   准入成功分支需要完整的发送尝试、凭据与资金事实，尚无夹具，留作后续覆盖。
//!
//! 仅在 `test-db` feature 下编译，入口见 `just test-db`。
#![cfg(feature = "test-db")]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::BTreeSet;
use std::sync::Arc;

use auction_application::auction::{
    AuctionLifecycleAction, AuctionLifecycleActor, AuctionLifecycleActorType,
    AuctionLifecycleCommand, AuctionLifecycleCommandPort, AuctionLifecyclePolicy,
    AuctionPolicyTemplate, CompetitionCommandPort, PlaceBidCommand,
};
use auction_application::core::RequestHash;
use auction_application::delivery::{
    DeliverySentConfirmationPort, ManualSentEvidence, SentConfirmationCommand,
};
use auction_application::recovery::RecoveryWorkStore;
use auction_application::settlement::{
    SettleAuctionCommand, SettlementCommandPort, SettlementWorkClaim, SettlementWorkStore,
    settlement_complete_request_hash,
};
use auction_domain::{
    AuctionId, BidAmount, CommandId, LotId, SettlementObligationKind, SettlementObligationState,
    SettlementPlan, UserId,
};
use auction_storage::{
    DatabaseAuctionLifecycleCommands, DatabaseCompetitionCommands, DatabaseSettlementCommands,
    DatabaseStore,
};
use chrono::{DateTime, Duration, Timelike, Utc};
use uuid::Uuid;

use crate::support;

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
    deadline: DateTime<Utc>,
}

fn stable_id(suffix: &str) -> Uuid {
    Uuid::new_v5(&Uuid::NAMESPACE_OID, format!("{SEED}-{suffix}").as_bytes())
}

/// 用生产模板生成冻结策略快照，避免夹具与真实快照格式漂移；0 ppm 让最低加价固定为 30k。
fn policy(deadline: DateTime<Utc>) -> serde_json::Value {
    let template = AuctionPolicyTemplate {
        start_price: START_PRICE,
        bid_increment_ppm: 0,
        ..AuctionPolicyTemplate::default()
    };
    template.validate().expect("测试策略模板有效");
    template.snapshot(deadline)
}

async fn seed_user(pool: &sqlx::PgPool, user_id: Uuid, username: &str) {
    let mut public_key_hash = [0_u8; 32];
    public_key_hash[..16].copy_from_slice(stable_id(&format!("{username}-key")).as_bytes());
    public_key_hash[16..].copy_from_slice(stable_id(&format!("{username}-salt")).as_bytes());
    sqlx::query("INSERT INTO public.users (id, username, public_key_hash) VALUES ($1, $2, $3)")
        .bind(user_id)
        .bind(username)
        .bind(public_key_hash.as_slice())
        .execute(pool)
        .await
        .expect("写入测试用户");
}

/// 写入一场 live 场次与两件带寄售来源的 lot；只写夹具，不经过被测端口。
async fn seed(admin: &DatabaseStore) -> Seeded {
    let pool = admin.postgres_pool();
    let deadline = Utc::now() + Duration::hours(2);
    let opened = deadline - Duration::hours(4);
    let auction_id = stable_id("auction");
    sqlx::query(
        "INSERT INTO public.auctions
            (id, world_id, sequence, title, status, scheduled_open_at, preview_scheduled_at,
             scheduled_end_at, effective_end_at, preview_opened_at, opened_at, policy_snapshot)
         VALUES ($1, 'main', 1, 'settlement e2e', 'live', $2, $2, $3, $3, $2, $2, $4)",
    )
    .bind(auction_id)
    .bind(opened)
    .bind(deadline)
    .bind(policy(deadline))
    .execute(pool)
    .await
    .expect("写入测试场次");
    sqlx::query("INSERT INTO public.realtime_streams (stream_key, next_sequence) VALUES ($1, 1)")
        .bind(format!("auction:{auction_id}"))
        .execute(pool)
        .await
        .expect("写入测试 stream");

    let bidders = [stable_id("bidder-0"), stable_id("bidder-1")];
    for (index, bidder) in bidders.iter().enumerate() {
        seed_user(pool, *bidder, &format!("{SEED}-bidder-{index}")).await;
    }

    let mut lots = Vec::with_capacity(2);
    for index in 0..2_i32 {
        let equipment_id = stable_id(&format!("equipment-{index}"));
        sqlx::query(
            "INSERT INTO public.equipment
                (id, world_id, external_equipment_id, name, category, attributes, collected_at)
             VALUES ($1, 'main', $2, $3, 'weapon', '{}'::jsonb, $4)",
        )
        .bind(equipment_id)
        .bind(format!("{SEED}-eid-{index}"))
        .bind(format!("{SEED} equipment {index}"))
        .bind(opened)
        .execute(pool)
        .await
        .expect("写入测试装备");

        // 结算需要卖方身份：每件 lot 挂一条 Admin 来源的寄售记录。
        let submission_id = stable_id(&format!("submission-{index}"));
        let command_id = stable_id(&format!("submission-command-{index}"));
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

        let lot_id = stable_id(&format!("lot-{index}"));
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
        command_id: CommandId::from_uuid(stable_id(&format!("bid-{round}"))),
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
        command_id: CommandId::from_uuid(stable_id("final-phase")),
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
    now: DateTime<Utc>,
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
    let latest_end: DateTime<Utc> = sqlx::query_scalar(
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
    let admin = database.store().await.expect("以维护账号连接");
    let service = database.service_store().await.expect("以运行账号连接");
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

async fn recovery_row(
    pool: &sqlx::PgPool,
    auction_id: Uuid,
) -> (Uuid, String, i64, i32, DateTime<Utc>, Option<DateTime<Utc>>) {
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

/// W14.6（D9）：付款期限已过但付款邮件没有发送证明时，恢复推进只延后并审计，不创建逾期提醒、
/// 不改冻结的付款期限；延后窗口内再次推进是空操作。起点是真实结算建立的恢复记录。
#[tokio::test]
async fn postgres_recovery_defers_overdue_reminder_without_payment_mail_proof_as_runtime_account() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let admin = database.store().await.expect("以维护账号连接");
    let service = database.service_store().await.expect("以运行账号连接");
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
        Utc::now().with_second(0).and_then(|value| value.with_nanosecond(0)).expect("整分钟");
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

/// W3.1：人工 Sent 证明的准入以运行账号读取完整锁定快照并由纯函数判定；不满足准入的投递
/// 只返回阻断项，`confirm_sent` 失败关闭且不追加任何证明事实或改变投递状态。
#[tokio::test]
async fn postgres_manual_sent_confirmation_rejects_inadmissible_delivery_as_runtime_account() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let admin = database.store().await.expect("以维护账号连接");
    let service = database.service_store().await.expect("以运行账号连接");
    let seeded = seed(&admin).await;
    let pool = admin.postgres_pool();
    let Settled { claim, now, .. } = settle_through_ports(&admin, &service, &seeded).await;
    service.complete(claim, now).await.expect("完成结算 workflow");

    // 不存在的投递：只给出阻断项。
    let missing = service
        .preview_sent_confirmation(Uuid::new_v4(), manual_evidence())
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
        .preview_sent_confirmation(delivery_id, manual_evidence())
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
    service.confirm_sent(command).await.expect_err("不满足准入时必须失败关闭");

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
