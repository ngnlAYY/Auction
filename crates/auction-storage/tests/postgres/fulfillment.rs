//! Public 履约进度读取的 PostgreSQL 18 用例，被测读取全程以运行账号执行。
//!
//! 夹具只用 SQL 写入场次、寄售来源、装备、lot 与用户；出价、进入 Final、领取与结算走生产端口，
//! 得到真实写入的成交与流拍事实（含一件撤拍 lot）。之后用维护账号推进少量履约事实，并经
//! `RecoveryWorkStore::record_due_return` 记录一封 COD 退件，覆盖已寄出、已完成与退件三类分支。
//!
//! 覆盖：
//! - 未结算场次、世界不符与不存在的场次都返回 `None`；
//! - 可见集合只有成交与流拍 lot，撤拍 lot 不出现；排序、分页、total 与 lot 筛选同口径；
//! - 刚结算时成交 lot 为 `payment_mail_pending`、流拍 lot 为 `unsold_return_pending`，买家、
//!   成交价与卖家取自结算事实；
//! - 推进后依次得到 `awaiting_payment`（含付款期限）、`returned_by_buyer`（含退回次数与上限）、
//!   `payout_pending`、`unsold_return_sent` 与 `unsold_returned`。
//!
//! 仅在 `test-db` feature 下编译，入口见 `just test-db`。
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
    PublicFulfillmentRequest, PublicFulfillmentStatus,
};
use auction_application::recovery::RecoveryWorkStore;
use auction_application::settlement::{
    SettleAuctionCommand, SettlementCommandPort, SettlementWorkStore,
    settlement_complete_request_hash,
};
use auction_domain::{AuctionId, BidAmount, CommandId, LotId, UserId};
use auction_storage::{
    DatabaseAuctionLifecycleCommands, DatabaseCompetitionCommands, DatabaseSettlementCommands,
    DatabaseStore,
};
use chrono::{DateTime, Duration, Utc};
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
    deadline: DateTime<Utc>,
}

/// 写入一场 live 场次、两名竞买人与四件带寄售来源的 lot；只写夹具，不经过被测端口。
async fn seed(admin: &DatabaseStore) -> Seeded {
    let pool = admin.postgres_pool();
    let deadline = Utc::now() + Duration::hours(2);
    let opened = deadline - Duration::hours(4);
    let auction_id = stable_id(SEED, "auction");
    sqlx::query(
        "INSERT INTO public.auctions
            (id, world_id, sequence, title, status, scheduled_open_at, preview_scheduled_at,
             scheduled_end_at, effective_end_at, preview_opened_at, opened_at, policy_snapshot)
         VALUES ($1, 'main', 1, 'fulfillment', 'live', $2, $2, $3, $3, $2, $2, $4)",
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

    let mut lots = Vec::with_capacity(LOTS.len());
    let mut external_ids = Vec::with_capacity(LOTS.len());
    for (index, (category, category_number, status)) in LOTS.into_iter().enumerate() {
        let index = i32::try_from(index).expect("夹具序号");
        let equipment_id = stable_id(SEED, &format!("equipment-{index}"));
        let external_id = format!("{}", 9_200_000 + index);
        sqlx::query(
            "INSERT INTO public.equipment
                (id, world_id, external_equipment_id, name, category, attributes, collected_at)
             VALUES ($1, 'main', $2, $3, $4, '{}'::jsonb, $5)",
        )
        .bind(equipment_id)
        .bind(&external_id)
        .bind(format!("{SEED} equipment {index}"))
        .bind(category)
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
async fn settle_through_ports(service: &DatabaseStore, seeded: &Seeded) -> DateTime<Utc> {
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
/// 夹具的结算时刻在截止之后（未来），履约行的时间戳只前进不后退，保持 `updated_at >= created_at`。
async fn mark_sent(pool: &PgPool, obligation_id: Uuid, external_mail_id: u32) {
    let delivery_id = stable_id(SEED, &format!("delivery-{obligation_id}"));
    let payload = json!({
        "schema_version": 1,
        "operation": "send",
        "world": "main",
        "template": "settlement_equipment_handoff",
        "variables": { "reference": obligation_id.to_string() },
        "credits": 0,
        "cod": 0,
        "equipment_secret_refs": [],
    });
    sqlx::query(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload, status,
             completed_at, external_reference)
         VALUES ($1, $2, decode(repeat('55', 32), 'hex'), 'settlement', $3, 'FixtureRecipient',
                 $4::jsonb, 'succeeded', now(), $5)",
    )
    .bind(delivery_id)
    .bind(stable_id(SEED, &format!("delivery-command-{obligation_id}")))
    .bind(obligation_id.to_string())
    .bind(payload.to_string())
    .bind(format!("mooglemail:main:send:{external_mail_id}"))
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

/// 写入一封已接受的 COD 退件：附件是原装备，没有 Credits 与 COD。
async fn insert_return_mail(
    pool: &PgPool,
    payment_obligation_id: Uuid,
    external_equipment_id: &str,
    received_at: DateTime<Utc>,
) {
    sqlx::query(
        "INSERT INTO public.inbound_mail
            (id, mailbox_key, external_mail_id, world_id, mail_kind, content_digest,
             received_at, status, decision_code, decision_metadata, processed_at)
         VALUES ($1, 'fulfillment-returns', '880001', 'main', 'delivery', $2, $3, 'accepted',
                 'settlement_recovery_returned', $4, $3 + interval '1 minute')",
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
    .execute(pool)
    .await
    .expect("写入 COD 退件");
}

#[tokio::test]
async fn postgres_public_fulfillment_reads_settled_lots_from_settlement_facts() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let admin = database.maintenance_store().await.expect("以维护账号连接");
    let service = database.store().await.expect("以运行账号连接");
    let pool = admin.postgres_pool();
    let seeded = seed(&admin).await;

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
    sqlx::query(
        "UPDATE public.settlement_obligations SET business_state = 'ready'
         WHERE lot_id = $1 AND obligation_kind = 'seller_payout'",
    )
    .bind(seeded.lots[0])
    .execute(pool)
    .await
    .expect("货款随付款解锁");
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
    let deadline: DateTime<Utc> = sqlx::query_scalar(
        "SELECT payment_deadline_at FROM public.settlement_recoveries WHERE lot_id = $1",
    )
    .bind(seeded.lots[1])
    .fetch_one(pool)
    .await
    .expect("读取付款期限");
    let views = page.views().expect("可判定");
    assert_eq!(views[1].payment_deadline_at, Some(deadline), "等待付款时公开当前期限");
    assert_eq!(views[0].payment_deadline_at, None);

    // 生产端口记录一封 COD 退件：recovery 进入退件阶段，退回次数加一。
    insert_return_mail(pool, sent_cod, &seeded.external_ids[1], settled_at + Duration::minutes(1))
        .await;
    let advance = service
        .record_due_return(settled_at + Duration::minutes(5))
        .await
        .expect("记录退件")
        .expect("领取到退件");
    assert!(advance.blocking_code.is_none(), "证据相符的退件不阻断");
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
