//! Public 履约进度读取的 PostgreSQL 18 用例，被测读取全程以运行账号执行。
//!
//! 夹具只用 SQL 写入场次、寄售来源、装备、lot 与用户；出价、进入 Final、领取与结算走生产端口，
//! 得到真实写入的成交与流拍事实（含一件撤拍 lot）。之后用维护账号推进少量履约事实（投递成功、
//! 履约满足、前置满足后的解锁、回收动作成功），并经 `RecoveryWorkStore::record_due_return` 记录
//! COD 退件、经 `RecoveryResolutionStore::advance_due_cod_return` 重寄或撤销，覆盖已寄出、已完成、
//! 退件、重寄与撤销寄回各类分支。
//!
//! 覆盖：
//! - 未结算场次、世界不符与不存在的场次都返回 `None`；
//! - 可见集合只有成交与流拍 lot，撤拍 lot 不出现；排序、分页、total 与 lot 筛选同口径；
//! - 刚结算时成交 lot 为 `payment_mail_pending`、流拍 lot 为 `unsold_return_pending`，买家、
//!   成交价与卖家取自结算事实；
//! - 推进后依次得到 `awaiting_payment`（含付款期限）、`returned_by_buyer`（含退回次数与上限）、
//!   `payout_pending`、`unsold_return_sent` 与 `unsold_returned`；
//! - 异世界成交沿主世界载体 COD → 异世界交付 → 主世界货款推进到 `completed`；
//! - COD 退回后重寄：`cod_resend_pending` → `cod_resent_awaiting_payment`（期限按重寄时刻重新起算）；
//! - 退回达到冻结上限：`cod_return_limit_reached` → 成交撤销后的 `voided_return_pending` →
//!   `voided_return_sent` → `voided_returned`。
//!
//! 仅在 `test-db` feature 下编译，入口见 `mise run test-db`。
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;

use auction_application::auction::{
    AuctionLifecycleAction, AuctionLifecycleActor, AuctionLifecycleActorType,
    AuctionLifecycleCommand, AuctionLifecycleCommandPort, AuctionLifecyclePolicy,
    AuctionPolicyTemplate, CompetitionCommandPort, PlaceBidCommand,
};
use auction_application::core::RequestHash;
use auction_application::public::{
    PublicFulfillmentCodReturns, PublicFulfillmentPage, PublicFulfillmentReader,
    PublicFulfillmentRequest, PublicFulfillmentStatus, PublicFulfillmentView,
};
use auction_application::recovery::{RecoveryResolutionStore, RecoveryWorkStore};
use auction_application::settlement::{
    SettleAuctionCommand, SettlementCommandPort, SettlementWorkStore,
    settlement_complete_request_hash,
};
use auction_domain::{AuctionId, BidAmount, CommandId, LotId, UserId};
use auction_storage::{
    DatabaseAuctionLifecycleCommands, DatabaseCompetitionCommands, DatabaseSettlementCommands,
    DatabaseStore,
};
use auction_time::{Duration, Timestamp};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use crate::support;
use crate::support::fixtures::{policy_snapshot, seed_user, stable_id};

const SEED: &str = "public-fulfillment";
const START_PRICE: i64 = 50_000;
/// 0 ppm 时冻结策略的统一最低加价（`POLICY_MINIMUM_INCREMENT`）。
const BID_STEP: i64 = 30_000;
const SELLER: &str = "fulfillment-seller";

/// 夹具 lot：装备分类、类别内编号与最终状态；撤拍 lot 不参与结算。
const LOTS: [(&str, i32, &str); 4] =
    [("Cloth", 1, "live"), ("Cloth", 2, "live"), ("Heavy", 1, "live"), ("Light", 1, "withdrawn")];

struct Seeded {
    auction_id: Uuid,
    lots: Vec<Uuid>,
    external_ids: Vec<String>,
    bidders: [Uuid; 2],
    deadline: Timestamp,
}

/// 在 `world` 写入一场 live 场次、两名竞买人与四件带寄售来源的 lot（装备与场次同一世界）；
/// 只写夹具，不经过被测端口。
async fn seed(admin: &DatabaseStore, world: &str) -> Seeded {
    let pool = admin.postgres_pool();
    let deadline = Timestamp::now() + Duration::hours(2);
    let opened = deadline - Duration::hours(4);
    let auction_id = stable_id(SEED, "auction");
    sqlx::query(
        "INSERT INTO public.auctions
            (id, world_id, sequence, title, status, scheduled_open_at, preview_scheduled_at,
             scheduled_end_at, effective_end_at, preview_opened_at, opened_at, policy_snapshot)
         VALUES ($1, $5, 1, 'fulfillment', 'live', $2, $2, $3, $3, $2, $2, $4)",
    )
    .bind(auction_id)
    .bind(opened)
    .bind(deadline)
    .bind(policy_snapshot(deadline, START_PRICE, BID_STEP))
    .bind(world)
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

    let mut lots = Vec::with_capacity(LOTS.len());
    let mut external_ids = Vec::with_capacity(LOTS.len());
    for (index, (category, category_number, status)) in LOTS.into_iter().enumerate() {
        let index = i32::try_from(index).expect("夹具序号");
        let equipment_id = stable_id(SEED, &format!("equipment-{index}"));
        let external_id = format!("{}", 9_200_000 + index);
        sqlx::query(
            "INSERT INTO public.equipment
                (id, world_id, external_equipment_id, name, category, attributes, collected_at)
             VALUES ($1, $6, $2, $3, $4, '{}'::jsonb, $5)",
        )
        .bind(equipment_id)
        .bind(&external_id)
        .bind(format!("{SEED} equipment {index}"))
        .bind(category)
        .bind(opened)
        .bind(world)
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
             VALUES ($1, $2, $3, $4, $5, $6, $6,
                     CASE WHEN $5 = 'live' THEN $7 END, $8, $9, $10)",
        )
        .bind(lot_id)
        .bind(auction_id)
        .bind(equipment_id)
        .bind(index + 1)
        .bind(status)
        .bind(START_PRICE)
        .bind(deadline)
        .bind(category)
        .bind(category_number)
        .bind(submission_id)
        .execute(pool)
        .await
        .expect("写入测试 lot");
        lots.push(lot_id);
        external_ids.push(external_id);
    }

    Seeded { auction_id, lots, external_ids, bidders, deadline }
}

fn bid(lot_uuid: Uuid, bidder_id: Uuid, amount: i64) -> PlaceBidCommand {
    let lot_id = LotId::from_uuid(lot_uuid);
    let bidder_id = UserId::from_uuid(bidder_id);
    let amount = BidAmount::new(amount).expect("合法竞价金额");
    PlaceBidCommand {
        command_id: CommandId::from_uuid(stable_id(SEED, &format!("bid-{lot_uuid}"))),
        lot_id,
        bidder_id,
        amount,
        request_hash: PlaceBidCommand::legacy_request_hash(lot_id, bidder_id, amount)
            .expect("稳定请求摘要"),
        request_id: Some(format!("{SEED}-bid-{lot_uuid}")),
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
            actor_id: "fulfillment-test-admin".to_owned(),
        },
    }
}

/// 经生产端口完成出价、进入 Final、领取与结算，全部以运行账号提交；返回冻结的结算时刻。
async fn settle_through_ports(service: &DatabaseStore, seeded: &Seeded) -> Timestamp {
    let bids = DatabaseCompetitionCommands::new(Arc::new(service.clone()));
    for (lot_id, bidder) in
        [(seeded.lots[0], seeded.bidders[0]), (seeded.lots[1], seeded.bidders[1])]
    {
        bids.place_bid(bid(lot_id, bidder, START_PRICE + BID_STEP))
            .await
            .unwrap_or_else(|error| panic!("出价必须被接受：{error}"));
    }
    let lifecycle = DatabaseAuctionLifecycleCommands::new(
        Arc::new(service.clone()),
        AuctionLifecyclePolicy::new(true, true),
        AuctionPolicyTemplate::default(),
    );
    lifecycle.execute(final_phase(seeded.auction_id)).await.expect("进入最终阶段");

    let now = seeded.deadline + Duration::seconds(1);
    let claim = service
        .claim_due("fulfillment-test-worker".to_owned(), now, now + Duration::minutes(5), 10)
        .await
        .expect("领取到期结算")
        .pop()
        .expect("结算领取");
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
    DatabaseSettlementCommands::new(Arc::new(service.clone()))
        .settle(command)
        .await
        .expect("结算必须以运行账号提交");
    let settled_at = claim.due_at;
    service.complete(claim, now).await.expect("完成结算 workflow");
    settled_at
}

fn request(auction_id: Uuid) -> PublicFulfillmentRequest {
    PublicFulfillmentRequest {
        auction_id,
        world_id: None,
        lot_id: None,
        lot_code: None,
        limit: 50,
        offset: 0,
    }
}

async fn read(service: &DatabaseStore, request: PublicFulfillmentRequest) -> PublicFulfillmentPage {
    service.list_public_fulfillment(request).await.expect("读取履约进度").expect("场次可见")
}

fn statuses(page: &PublicFulfillmentPage) -> Vec<PublicFulfillmentStatus> {
    page.views().expect("事实可判定").into_iter().map(|view| view.status).collect()
}

/// 指定 lot 某种义务的标识。
async fn obligation_id(pool: &PgPool, lot_id: Uuid, kind: &str) -> Uuid {
    sqlx::query_scalar(
        "SELECT id FROM public.settlement_obligations WHERE lot_id = $1 AND obligation_kind = $2",
    )
    .bind(lot_id)
    .bind(kind)
    .fetch_one(pool)
    .await
    .expect("读取结算义务")
}

/// 维护账号写入一条已成功的结算投递，并把义务履约改为已入队：对外即“已寄出、待对方领取”。
/// 投递世界取履约的派发世界（载体 COD 与货款在主世界，异世界交付在 isekai）；投递按领取过一次
/// 写入（`fencing_version = 1`），与生产寄出的投递同形状。夹具的结算时刻在截止之后（未来），
/// 履约行的时间戳只前进不后退，保持 `updated_at >= created_at`。
async fn mark_sent(pool: &PgPool, obligation_id: Uuid, external_mail_id: u32) {
    let world: String = sqlx::query_scalar(
        "SELECT dispatch_world FROM public.settlement_obligation_fulfillments
         WHERE obligation_id = $1",
    )
    .bind(obligation_id)
    .fetch_one(pool)
    .await
    .expect("读取履约派发世界");
    let delivery_id = stable_id(SEED, &format!("delivery-{obligation_id}"));
    let payload = json!({
        "schema_version": 1,
        "operation": "send",
        "world": world,
        "template": "settlement_equipment_handoff",
        "variables": { "reference": obligation_id.to_string() },
        "credits": 0,
        "cod": 0,
        "equipment_secret_refs": [],
    });
    sqlx::query(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload, status,
             fencing_version, attempt_count, claimed_at, sending_at, completed_at,
             external_reference)
         VALUES ($1, $2, decode(repeat('55', 32), 'hex'), 'settlement', $3, 'FixtureRecipient',
                 $4::jsonb, 'succeeded', 1, 1, now(), now(), now(), $5)",
    )
    .bind(delivery_id)
    .bind(stable_id(SEED, &format!("delivery-command-{obligation_id}")))
    .bind(obligation_id.to_string())
    .bind(payload.to_string())
    .bind(format!("mooglemail:{world}:send:{external_mail_id}"))
    .execute(pool)
    .await
    .expect("写入已成功的结算投递");
    sqlx::query(
        "UPDATE public.settlement_obligation_fulfillments
         SET execution_state = 'enqueued', delivery_id = $2,
             updated_at = greatest(updated_at, now())
         WHERE obligation_id = $1",
    )
    .bind(obligation_id)
    .bind(delivery_id)
    .execute(pool)
    .await
    .expect("履约改为已入队");
}

/// 维护账号把义务履约改为已满足（对方已领取）。
async fn mark_satisfied(pool: &PgPool, obligation_id: Uuid) {
    sqlx::query(
        "UPDATE public.settlement_obligation_fulfillments
         SET execution_state = 'satisfied', completed_at = greatest(updated_at, now()),
             updated_at = greatest(updated_at, now())
         WHERE obligation_id = $1",
    )
    .bind(obligation_id)
    .execute(pool)
    .await
    .expect("履约改为已满足");
}

/// 写入一封已接受的系统 COD 退件：附件是原装备，没有 Credits 与 COD。入站展示快照记录系统
/// 发件人与 `[RTS] ` + canonical 主题，恢复流程据此证明来源。
async fn insert_return_mail(
    pool: &PgPool,
    payment_obligation_id: Uuid,
    external_equipment_id: &str,
    received_at: Timestamp,
) {
    sqlx::query(
        "INSERT INTO public.inbound_mail
            (id, mailbox_key, external_mail_id, world_id, mail_kind, content_digest,
             received_at, status, decision_code, decision_metadata, processed_at,
             sender, subject)
         VALUES ($1, 'fulfillment-returns', '880001', 'main', 'delivery', $2, $3, 'accepted',
                 'settlement_recovery_returned', $4, $3 + interval '1 minute',
                 'MoogleMail', $5)",
    )
    .bind(stable_id(SEED, "return-mail"))
    .bind([6_u8; 32].as_slice())
    .bind(received_at)
    .bind(json!({
        "obligation_id": payment_obligation_id.to_string(),
        "credits": 0,
        "cod": 0,
        "attachment_ids": [external_equipment_id],
    }))
    .bind(format!("[RTS] Auction equipment delivery [{payment_obligation_id}]"))
    .execute(pool)
    .await
    .expect("写入 COD 退件");
}

/// 写入一封证据相符的 COD 退件，并经生产端口记录：recovery 进入退件阶段，退回次数加一。返回
/// 本次建立的恢复动作种类。
async fn record_return(
    service: &DatabaseStore,
    pool: &PgPool,
    payment_obligation_id: Uuid,
    external_equipment_id: &str,
    settled_at: Timestamp,
) -> Vec<String> {
    insert_return_mail(
        pool,
        payment_obligation_id,
        external_equipment_id,
        settled_at + Duration::minutes(1),
    )
    .await;
    let advance = service
        .record_due_return(settled_at + Duration::minutes(5))
        .await
        .expect("记录退件")
        .expect("领取到退件");
    assert!(advance.blocking_code.is_none(), "证据相符的退件不阻断");
    advance.action_kinds
}

/// 前置义务满足后由履约服务解锁后续义务；夹具以维护账号直接改为可执行。
async fn unblock(pool: &PgPool, lot_id: Uuid, kind: &str) {
    let updated = sqlx::query(
        "UPDATE public.settlement_obligations SET business_state = 'ready'
         WHERE lot_id = $1 AND obligation_kind = $2 AND business_state = 'blocked'",
    )
    .bind(lot_id)
    .bind(kind)
    .execute(pool)
    .await
    .expect("义务随前置满足解锁");
    assert_eq!(updated.rows_affected(), 1, "{kind} 应处于等待前置");
}

/// 维护账号模拟 Worker 完成一项恢复动作：动作满足并带证明；回收动作另把同一证明投影为 recovery
/// 的保管证明，与生产终态写入同形状。证明引用退件的外部邮件号，观察时刻落在 recovery 的创建与
/// 更新时间之间。
async fn complete_recovery_action(pool: &PgPool, lot_id: Uuid, kind: &str, observed_at: Timestamp) {
    let digest = [7_u8; 32];
    let reclaim = kind == "reclaim_equipment_take";
    // 回收动作的证明是退件装备取回成功；拉黑动作只需带证明，用夹具标签。
    let proof_kind =
        if reclaim { "delivery_recovery_take_succeeded" } else { "fixture_action_completed" };
    let completed = sqlx::query(
        "UPDATE public.settlement_recovery_actions AS action
         SET execution_state = 'satisfied',
             proof_kind = $5,
             proof_reference = recovery.return_external_mail_id,
             proof_digest = $3,
             proof_observed_at = $4,
             next_attempt_at = NULL,
             completed_at = greatest(action.updated_at, $4),
             state_version = action.state_version + 1,
             updated_at = greatest(action.updated_at, $4)
         FROM public.settlement_recoveries AS recovery
         WHERE recovery.lot_id = $1
           AND action.recovery_id = recovery.id
           AND action.kind = $2
           AND action.execution_state = 'pending'",
    )
    .bind(lot_id)
    .bind(kind)
    .bind(digest.as_slice())
    .bind(observed_at)
    .bind(proof_kind)
    .execute(pool)
    .await
    .expect("恢复动作改为已满足");
    assert_eq!(completed.rows_affected(), 1, "{kind} 应有一项待执行动作");
    if reclaim {
        sqlx::query(
            "UPDATE public.settlement_recoveries
             SET custody_proof_kind = 'delivery_recovery_take_succeeded',
                 custody_proof_reference = return_external_mail_id,
                 custody_proof_digest = $2,
                 custody_proof_observed_at = $3,
                 state_version = state_version + 1,
                 updated_at = greatest(updated_at, $3)
             WHERE lot_id = $1 AND stage = 'reclaiming'",
        )
        .bind(lot_id)
        .bind(digest.as_slice())
        .bind(observed_at)
        .execute(pool)
        .await
        .expect("写入 recovery 保管证明");
    }
}

/// 重寄把原投递重置为待发送；维护账号把它改为再次寄出成功，履约保持已入队。
async fn mark_resent(pool: &PgPool, obligation_id: Uuid, external_mail_id: u32) {
    let updated = sqlx::query(
        "UPDATE public.deliveries AS delivery
         SET status = 'succeeded',
             attempt_count = 1,
             claimed_at = delivery.updated_at,
             sending_at = delivery.updated_at,
             completed_at = delivery.updated_at,
             external_reference = $2
         FROM public.settlement_obligation_fulfillments AS fulfillment
         WHERE fulfillment.obligation_id = $1
           AND fulfillment.execution_state = 'enqueued'
           AND delivery.id = fulfillment.delivery_id
           AND delivery.status = 'pending'",
    )
    .bind(obligation_id)
    .bind(format!("mooglemail:main:send:{external_mail_id}"))
    .execute(pool)
    .await
    .expect("重寄投递改为已成功");
    assert_eq!(updated.rows_affected(), 1, "重寄后应有一条待发送的原投递");
}

/// 撤销成交后新建的撤拍寄回义务还没有履约行（生产由履约领取按派发语义补齐）；维护账号补写。
async fn insert_pending_fulfillment(pool: &PgPool, obligation_id: Uuid) {
    sqlx::query(
        "INSERT INTO public.settlement_obligation_fulfillments
             (obligation_id, execution_state, dispatch_world, delivery_operation)
         SELECT id, 'pending', world_id, 'send'
         FROM public.settlement_obligations WHERE id = $1",
    )
    .bind(obligation_id)
    .execute(pool)
    .await
    .expect("补写撤拍寄回履约行");
}

async fn recovery_deadline(pool: &PgPool, lot_id: Uuid) -> Timestamp {
    sqlx::query_scalar(
        "SELECT payment_deadline_at FROM public.settlement_recoveries WHERE lot_id = $1",
    )
    .bind(lot_id)
    .fetch_one(pool)
    .await
    .expect("读取付款期限")
}

/// 按 lot UUID 筛选读取单件的公开状态与附带字段。
async fn lot_view(
    service: &DatabaseStore,
    auction_id: Uuid,
    lot_id: Uuid,
) -> PublicFulfillmentView {
    let page =
        read(service, PublicFulfillmentRequest { lot_id: Some(lot_id), ..request(auction_id) })
            .await;
    assert_eq!(page.total, 1, "按 UUID 只筛出这一件");
    page.views().expect("事实可判定").remove(0)
}

#[tokio::test]
async fn postgres_public_fulfillment_reads_settled_lots_from_settlement_facts() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let admin = database.maintenance_store().await.expect("以维护账号连接");
    let service = database.store().await.expect("以运行账号连接");
    let pool = admin.postgres_pool();
    let seeded = seed(&admin, "main").await;

    // 进行中的场次没有履约进度。
    let live = service.list_public_fulfillment(request(seeded.auction_id)).await.expect("读取");
    assert!(live.is_none(), "未结算场次不可见");
    let settled_at = settle_through_ports(&service, &seeded).await;

    let page = read(&service, request(seeded.auction_id)).await;
    assert_eq!(page.auction.auction_id, seeded.auction_id);
    assert_eq!((page.auction.world_id.as_str(), page.auction.sequence), ("main", 1));
    assert_eq!(page.total, 3, "撤拍 lot 不在可见集合内");
    let lot_ids = page.entries.iter().map(|lot| lot.lot_id).collect::<Vec<_>>();
    assert_eq!(lot_ids, seeded.lots[..3], "按 lot 顺序排列");
    assert_eq!(
        statuses(&page),
        [
            PublicFulfillmentStatus::PaymentMailPending,
            PublicFulfillmentStatus::PaymentMailPending,
            PublicFulfillmentStatus::UnsoldReturnPending,
        ]
    );
    let views = page.views().expect("可判定");
    let first_sale = views[0].sale.as_ref().expect("成交事实");
    assert_eq!(first_sale.buyer_name, format!("{SEED}-bidder-0"));
    assert_eq!(first_sale.final_price, START_PRICE + BID_STEP);
    assert_eq!(views[2].sale, None, "流拍不公开买家与成交价");
    assert!(page.entries.iter().all(|lot| lot.seller_name == SELLER));
    assert_eq!(
        page.entries
            .iter()
            .map(|lot| (lot.equipment_category.as_str(), lot.lot_number))
            .collect::<Vec<_>>(),
        [("Cloth", 1), ("Cloth", 2), ("Heavy", 1)]
    );
    assert!(
        views.iter().all(|view| view.payment_deadline_at.is_none() && view.cod_returns.is_none())
    );

    // 世界不符或场次不存在时不可见。
    for request in [
        PublicFulfillmentRequest {
            world_id: Some("isekai".to_owned()),
            ..request(seeded.auction_id)
        },
        request(stable_id(SEED, "missing-auction")),
    ] {
        assert!(service.list_public_fulfillment(request).await.expect("读取").is_none());
    }
    let same_world = PublicFulfillmentRequest {
        world_id: Some("main".to_owned()),
        ..request(seeded.auction_id)
    };
    assert_eq!(read(&service, same_world).await.total, 3);

    // lot 筛选与分页沿用同一可见集合：total 只数匹配的 lot。
    let by_code = |code: &str| PublicFulfillmentRequest {
        lot_code: Some(code.to_owned()),
        ..request(seeded.auction_id)
    };
    let cloth = read(&service, by_code("clo")).await;
    assert_eq!((cloth.total, cloth.entries.len()), (2, 2));
    let heavy = read(&service, by_code("HEA01")).await;
    assert_eq!(heavy.entries.iter().map(|lot| lot.lot_id).collect::<Vec<_>>(), [seeded.lots[2]]);
    let withdrawn = read(&service, by_code("lig01")).await;
    assert_eq!((withdrawn.total, withdrawn.entries.len()), (0, 0), "撤拍 lot 不能被筛出");
    let by_id =
        PublicFulfillmentRequest { lot_id: Some(seeded.lots[1]), ..request(seeded.auction_id) };
    assert_eq!(read(&service, by_id).await.entries[0].lot_id, seeded.lots[1]);
    let second_page = read(
        &service,
        PublicFulfillmentRequest { limit: 2, offset: 2, ..request(seeded.auction_id) },
    )
    .await;
    assert_eq!((second_page.total, second_page.entries.len()), (3, 1));

    // 推进事实：成交 lot 0 的 COD 已付款、货款解锁；成交 lot 1 的 COD 已寄出；流拍 lot 已寄回。
    let paid_cod = obligation_id(pool, seeded.lots[0], "winner_equipment_cod").await;
    mark_sent(pool, paid_cod, 770_001).await;
    mark_satisfied(pool, paid_cod).await;
    unblock(pool, seeded.lots[0], "seller_payout").await;
    let sent_cod = obligation_id(pool, seeded.lots[1], "winner_equipment_cod").await;
    mark_sent(pool, sent_cod, 770_002).await;
    let unsold_return = obligation_id(pool, seeded.lots[2], "unsold_return").await;
    mark_sent(pool, unsold_return, 770_003).await;

    let page = read(&service, request(seeded.auction_id)).await;
    assert_eq!(
        statuses(&page),
        [
            PublicFulfillmentStatus::PayoutPending,
            PublicFulfillmentStatus::AwaitingPayment,
            PublicFulfillmentStatus::UnsoldReturnSent,
        ]
    );
    let deadline = recovery_deadline(pool, seeded.lots[1]).await;
    let views = page.views().expect("可判定");
    assert_eq!(views[1].payment_deadline_at, Some(deadline), "等待付款时公开当前期限");
    assert_eq!(views[0].payment_deadline_at, None);

    // 生产端口记录一封 COD 退件：recovery 进入退件阶段，退回次数加一。
    record_return(&service, pool, sent_cod, &seeded.external_ids[1], settled_at).await;
    mark_satisfied(pool, unsold_return).await;
    let limit: i32 = sqlx::query_scalar(
        "SELECT cod_return_limit FROM public.settlement_recoveries WHERE lot_id = $1",
    )
    .bind(seeded.lots[1])
    .fetch_one(pool)
    .await
    .expect("读取退回上限");

    let page = read(&service, request(seeded.auction_id)).await;
    assert_eq!(
        statuses(&page),
        [
            PublicFulfillmentStatus::PayoutPending,
            PublicFulfillmentStatus::ReturnedByBuyer,
            PublicFulfillmentStatus::UnsoldReturned,
        ]
    );
    let views = page.views().expect("可判定");
    assert_eq!(views[1].cod_returns, Some(PublicFulfillmentCodReturns { count: 1, limit }));
    assert_eq!(views[1].payment_deadline_at, None, "退件阶段不公开付款期限");

    service.postgres_pool().close().await;
    admin.postgres_pool().close().await;
}

/// 异世界成交：主世界寄出的载体 COD 收口后，异世界交付装备，再由主世界发放货款，逐步推进到完成。
#[tokio::test]
async fn postgres_public_fulfillment_follows_the_isekai_carrier_handoff_and_payout() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let admin = database.maintenance_store().await.expect("以维护账号连接");
    let service = database.store().await.expect("以运行账号连接");
    let pool = admin.postgres_pool();
    let seeded = seed(&admin, "isekai").await;
    settle_through_ports(&service, &seeded).await;
    let auction_id = seeded.auction_id;
    let lot = seeded.lots[0];

    let page = read(&service, request(auction_id)).await;
    assert_eq!(page.auction.world_id, "isekai");
    assert_eq!(
        statuses(&page),
        [
            PublicFulfillmentStatus::PaymentMailPending,
            PublicFulfillmentStatus::PaymentMailPending,
            PublicFulfillmentStatus::UnsoldReturnPending,
        ]
    );
    let main_only =
        PublicFulfillmentRequest { world_id: Some("main".to_owned()), ..request(auction_id) };
    assert!(
        service.list_public_fulfillment(main_only).await.expect("读取").is_none(),
        "世界约束与场次世界不符时不可见"
    );

    // 主世界寄出载体 COD：等待确认收款，公开当前付款期限。
    let carrier = obligation_id(pool, lot, "winner_payment_carrier_cod").await;
    mark_sent(pool, carrier, 771_001).await;
    let view = lot_view(&service, auction_id, lot).await;
    assert_eq!(view.status, PublicFulfillmentStatus::AwaitingPayment);
    assert_eq!(view.payment_deadline_at, Some(recovery_deadline(pool, lot).await));

    // 载体付款收口后异世界交付解锁：待交付 → 已寄出待买家领取。
    mark_satisfied(pool, carrier).await;
    unblock(pool, lot, "winner_equipment_handoff").await;
    let view = lot_view(&service, auction_id, lot).await;
    assert_eq!(view.status, PublicFulfillmentStatus::HandoffPending);
    assert_eq!(view.payment_deadline_at, None, "付款收口后不再公开期限");
    let handoff = obligation_id(pool, lot, "winner_equipment_handoff").await;
    mark_sent(pool, handoff, 771_002).await;
    assert_eq!(
        lot_view(&service, auction_id, lot).await.status,
        PublicFulfillmentStatus::HandoffSent
    );

    // 买家领取装备后货款解锁：待发放 → 已寄出 → 卖家领取即完成。
    mark_satisfied(pool, handoff).await;
    unblock(pool, lot, "seller_payout").await;
    assert_eq!(
        lot_view(&service, auction_id, lot).await.status,
        PublicFulfillmentStatus::PayoutPending
    );
    let payout = obligation_id(pool, lot, "seller_payout").await;
    mark_sent(pool, payout, 771_003).await;
    assert_eq!(
        lot_view(&service, auction_id, lot).await.status,
        PublicFulfillmentStatus::PayoutSent
    );
    mark_satisfied(pool, payout).await;
    let view = lot_view(&service, auction_id, lot).await;
    assert_eq!(view.status, PublicFulfillmentStatus::Completed);
    assert_eq!(view.sale.map(|sale| sale.final_price), Some(START_PRICE + BID_STEP));
    assert_eq!((view.payment_deadline_at, view.cod_returns), (None, None));

    // 其余 lot 不受这件推进影响。
    let page = read(&service, request(auction_id)).await;
    assert_eq!(
        statuses(&page)[1..],
        [PublicFulfillmentStatus::PaymentMailPending, PublicFulfillmentStatus::UnsoldReturnPending]
    );

    service.postgres_pool().close().await;
    admin.postgres_pool().close().await;
}

/// COD 被退回、装备收回后，生产端口按冻结策略重寄原 COD：待重新寄出 → 已重寄、待确认收款，
/// 付款期限从重寄时刻重新起算，退回次数一路成对公开。
#[tokio::test]
async fn postgres_public_fulfillment_reports_the_cod_resend_after_a_return() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let admin = database.maintenance_store().await.expect("以维护账号连接");
    let service = database.store().await.expect("以运行账号连接");
    let pool = admin.postgres_pool();
    let seeded = seed(&admin, "main").await;
    let settled_at = settle_through_ports(&service, &seeded).await;
    let auction_id = seeded.auction_id;
    let lot = seeded.lots[1];
    let cod = obligation_id(pool, lot, "winner_equipment_cod").await;
    mark_sent(pool, cod, 772_001).await;
    let first_deadline = recovery_deadline(pool, lot).await;

    let actions = record_return(&service, pool, cod, &seeded.external_ids[1], settled_at).await;
    assert_eq!(actions, ["reclaim_equipment_take"], "未达上限只回收装备");
    let limit: i32 = sqlx::query_scalar(
        "SELECT cod_return_limit FROM public.settlement_recoveries WHERE lot_id = $1",
    )
    .bind(lot)
    .fetch_one(pool)
    .await
    .expect("读取退回上限");
    assert!(limit > 1, "默认上限允许重寄");
    let returns = Some(PublicFulfillmentCodReturns { count: 1, limit });
    let view = lot_view(&service, auction_id, lot).await;
    assert_eq!(
        (view.status, view.cod_returns),
        (PublicFulfillmentStatus::ReturnedByBuyer, returns)
    );

    // 回收动作成功、保管证明落到 recovery 后，生产端口重置原 COD 投递并回到等待付款。
    complete_recovery_action(
        pool,
        lot,
        "reclaim_equipment_take",
        settled_at + Duration::minutes(6),
    )
    .await;
    let resend_at = settled_at + Duration::minutes(10);
    let advance = service
        .advance_due_cod_return(resend_at)
        .await
        .expect("重寄 COD")
        .expect("领取到待重寄的 recovery");
    assert_eq!((advance.stage.as_str(), advance.resolution), ("awaiting_payment", None));
    let view = lot_view(&service, auction_id, lot).await;
    assert_eq!(view.status, PublicFulfillmentStatus::CodResendPending);
    assert_eq!(view.cod_returns, returns);
    assert_eq!(view.payment_deadline_at, None, "尚未重新寄出时不公开期限");

    // 原投递再次寄出成功：已重寄、待确认收款，公开从重寄时刻重新起算的期限。
    mark_resent(pool, cod, 772_002).await;
    let deadline = recovery_deadline(pool, lot).await;
    assert!(deadline > first_deadline && deadline > resend_at, "期限从重寄时刻重新起算");
    let view = lot_view(&service, auction_id, lot).await;
    assert_eq!(view.status, PublicFulfillmentStatus::CodResentAwaitingPayment);
    assert_eq!(view.payment_deadline_at, Some(deadline));
    assert_eq!(view.cod_returns, returns);

    service.postgres_pool().close().await;
    admin.postgres_pool().close().await;
}

/// COD 退回达到冻结上限：退件处理中显示达上限；装备收回、买家拉黑后，生产端口撤销成交并建立
/// 撤拍寄回义务，公开状态随寄回推进：待寄回 → 已寄回待卖家领取 → 卖家已取回。
#[tokio::test]
async fn postgres_public_fulfillment_follows_the_return_to_seller_after_the_cod_limit() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let admin = database.maintenance_store().await.expect("以维护账号连接");
    let service = database.store().await.expect("以运行账号连接");
    let pool = admin.postgres_pool();
    let seeded = seed(&admin, "main").await;
    let settled_at = settle_through_ports(&service, &seeded).await;
    let auction_id = seeded.auction_id;
    let lot = seeded.lots[1];
    // 夹具把这件成交冻结的退回上限改为 1：第一次退件即达上限，生产端口随之建立拉黑动作。
    sqlx::query("UPDATE public.settlement_recoveries SET cod_return_limit = 1 WHERE lot_id = $1")
        .bind(lot)
        .execute(pool)
        .await
        .expect("冻结退回上限为 1");
    let cod = obligation_id(pool, lot, "winner_equipment_cod").await;
    mark_sent(pool, cod, 773_001).await;

    let actions = record_return(&service, pool, cod, &seeded.external_ids[1], settled_at).await;
    assert_eq!(actions, ["reclaim_equipment_take", "buyer_blacklist"]);
    let returns = Some(PublicFulfillmentCodReturns { count: 1, limit: 1 });
    let view = lot_view(&service, auction_id, lot).await;
    assert_eq!(
        (view.status, view.cod_returns),
        (PublicFulfillmentStatus::CodReturnLimitReached, returns)
    );

    let observed_at = settled_at + Duration::minutes(6);
    complete_recovery_action(pool, lot, "reclaim_equipment_take", observed_at).await;
    complete_recovery_action(pool, lot, "buyer_blacklist", observed_at).await;
    let advance = service
        .advance_due_cod_return(settled_at + Duration::minutes(10))
        .await
        .expect("撤销成交并寄回卖家")
        .expect("领取到达上限的 recovery");
    assert_eq!(advance.resolution.as_deref(), Some("returned_after_three_cod_returns"));
    let view = lot_view(&service, auction_id, lot).await;
    assert_eq!(view.status, PublicFulfillmentStatus::VoidedReturnPending, "寄回履约尚未建立");
    assert_eq!(view.cod_returns, returns);
    assert!(view.sale.is_some(), "撤销后仍公开原成交事实");

    let withdrawn = obligation_id(pool, lot, "withdrawn_return").await;
    insert_pending_fulfillment(pool, withdrawn).await;
    assert_eq!(
        lot_view(&service, auction_id, lot).await.status,
        PublicFulfillmentStatus::VoidedReturnPending
    );
    mark_sent(pool, withdrawn, 773_002).await;
    assert_eq!(
        lot_view(&service, auction_id, lot).await.status,
        PublicFulfillmentStatus::VoidedReturnSent
    );
    mark_satisfied(pool, withdrawn).await;
    assert_eq!(
        lot_view(&service, auction_id, lot).await.status,
        PublicFulfillmentStatus::VoidedReturned
    );

    service.postgres_pool().close().await;
    admin.postgres_pool().close().await;
}
