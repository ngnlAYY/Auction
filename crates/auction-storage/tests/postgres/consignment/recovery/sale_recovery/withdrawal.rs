//! Sale 已占代 g 后的直接撤回：Sale 已封闭且原件在库 → 追加 g+1 `withdrawal_only` 代；旧买方交割未封闭或
//! 原件可能已发出未退回 → 拒绝，且不留任何痕迹。
//!
//! 保护的不变量：1（同命令重放原回执）、2（唯一后继与指针 CAS）、3（代、合同、事件、回执同提）、
//! 4（原件可能在途时不放行）、6（失败关闭）。

use std::sync::Arc;

use auction_application::consignment::{
    ConsignmentError, CustodyControlStore, CustodyRelistStore, DepositBusinessRejection,
    DispatchService, DispatchWorkStore, ListingDelistReason, RelistCommand,
    SetOperationsHoldCommand, WithdrawDepositCommand, WriteStart,
};
use auction_application::core::RequestHash;
use auction_application::delivery::DeliverySendCredentialBinding;
use auction_domain::consignment::dispatch::{AttemptKey, dispatch_round_id};
use auction_domain::consignment::{FeeBps, ListingPrice, ListingState, ListingVersion};
use auction_storage::{DepositRepository, DepositSettings, DispatchWorkRepository};
use auction_time::{Duration, Timestamp};

use super::super::super::contracts::fixture::CONTRACT;
use super::super::super::deposit::support::{
    CREDENTIAL, LISTING, POSITION, REPLAY, USER_1, succeeds,
};
use super::super::super::ledger136::fixture::original_dispatch_sql;
use super::fixture::FakeContent;
use super::{Closure, World, closure_sql, command_id, id, seller, unknown_refund_sql};

/// 同一卖家另一件占活动额度的挂单（resolving）与它的登记命令；明显的假 UUID。
const OTHER_LISTING: &str = "c7f40000-0000-4000-8000-000000000001";
const OTHER_REGISTER: &str = "c7f40000-0000-4000-8000-000000000002";

fn withdraw(seed: u128, version: u64, hash: u8) -> WithdrawDepositCommand {
    WithdrawDepositCommand {
        command_id: command_id(seed),
        seller_user_id: seller(),
        listing_id: id(LISTING),
        expected_version: ListingVersion::new(version).expect("版本"),
        request_hash: RequestHash::new([hash; 32]),
        request_id: None,
    }
}

/// 正例：买家未付、Sale 已封闭、首派从未起写（原件从未离库）。同事务回库、追加 g+1 撤回代、指针前进、
/// Withdrawal 带前驱代与封闭证明、挂单 `withdrawal_pending`；旧 Sale 与封闭事实原样保留，
/// 不产生任何 `available` 事件、新 Sale 或资金分录；同命令重放返回原回执且不增行，同 ID 异请求冲突。
#[tokio::test]
async fn postgres_sale_withdrawal_appends_withdrawal_only_generation_after_a_sealed_sale() {
    let world = World::sold(Closure::CancelledUnpaid).await;
    let available_before = world.listing_events("available").await;
    assert_eq!(world.generations().await.len(), 1);
    let command = withdraw(1, 6, 0x31);

    let receipt = world.deposit.request_withdrawal(command.clone()).await.expect("撤回");

    assert_eq!(receipt.disposition.as_str(), "withdrawal_accepted");
    assert_eq!(
        (receipt.state, receipt.listing_version.value()),
        (ListingState::WithdrawalPending, 7)
    );
    assert_eq!(
        world.generations().await,
        vec![(1, "first_sale".to_owned(), None), (2, "withdrawal_only".to_owned(), Some(1)),]
    );
    // 回库一次（版本 3 -> 4）、指针前进一次（4 -> 5）；位置回到在库且指针在 g+1。
    assert_eq!(world.position().await, ("in_custody".to_owned(), Some(2), 5));
    let (kind, closure): (String, Option<uuid::Uuid>) = sqlx::query_as(
        "SELECT event_kind, proof_id FROM public.consignment_custody_events
         WHERE position_id = $1 AND version = 4",
    )
    .bind(id(POSITION))
    .fetch_one(&world.pool)
    .await
    .expect("回库事件");
    assert_eq!((kind.as_str(), closure), ("uncommitted", Some(id(CONTRACT))));
    let (generation, predecessor_contract, predecessor_generation, proof_ref): (
        i64,
        Option<uuid::Uuid>,
        Option<i64>,
        Option<String>,
    ) = sqlx::query_as(
        "SELECT c.generation, w.predecessor_contract_id, w.predecessor_generation, w.closure_proof_ref
         FROM public.consignment_contracts c
         JOIN public.consignment_withdrawals w ON w.contract_id = c.contract_id",
    )
    .fetch_one(&world.pool)
    .await
    .expect("Withdrawal");
    assert_eq!(
        (generation, predecessor_contract, predecessor_generation),
        (2, Some(id(CONTRACT)), Some(1))
    );
    assert_eq!(
        proof_ref.as_deref(),
        Some(format!("sale-closure:cancelled_unpaid:{CONTRACT}").as_str())
    );
    // 旧 Sale 与封闭事实仍可查；没有新 Sale、新退款或资金分录；没有 available 事件。
    assert_eq!(world.count("consignment_sales").await, 1);
    assert_eq!(world.count("consignment_sale_closures").await, 1);
    assert_eq!(world.count("settlement_funding_entries").await, 0);
    assert_eq!(world.listing_events("available").await, available_before);
    assert_eq!(world.listing().await, ("withdrawal_pending".to_owned(), 7));

    let rows =
        (world.count("consignment_contracts").await, world.count("consignment_commands").await);
    let replay = world.deposit.request_withdrawal(command).await.expect("同命令重放");
    assert_eq!(replay, receipt);
    assert_eq!(
        (world.count("consignment_contracts").await, world.count("consignment_commands").await),
        rows
    );
    let conflict = world.deposit.request_withdrawal(withdraw(1, 6, 0x32)).await;
    assert!(matches!(conflict, Err(ConsignmentError::CommandIdConflict)), "{conflict:?}");
    assert_eq!(world.generations().await.len(), 2);
}

/// 反例：Sale 未封闭、或虽已封闭但原件已发出而未被精确收回（可能已交付买家）。两种情形都以
/// `equipment_committed` 拒绝；没有追加处分代、合同、事件，position 与挂单原样不变。
#[tokio::test]
async fn postgres_sale_withdrawal_refuses_an_open_sale_and_an_unreturned_original() {
    let open = World::sold(Closure::None).await;
    let before = (open.position().await, open.listing().await);
    let refused = open.deposit.request_withdrawal(withdraw(2, 6, 0x33)).await;
    assert!(
        matches!(
            refused,
            Err(ConsignmentError::Deposit(DepositBusinessRejection::EquipmentCommitted))
        ),
        "{refused:?}"
    );
    assert_eq!((open.position().await, open.listing().await), before);
    assert_eq!(open.generations().await.len(), 1);
    assert_eq!(open.count("consignment_withdrawals").await, 0);

    // 封闭事实在，但原件腿已有 Send 尝试且没有对应的精确 Take 消费。
    let sent = World::sold(Closure::None).await;
    succeeds(&sent.owner, &original_dispatch_sql()).await;
    succeeds(&sent.owner, &closure_sql("cancelled_unpaid")).await;
    let before = (sent.position().await, sent.listing().await);
    let refused = sent.deposit.request_withdrawal(withdraw(3, 6, 0x34)).await;
    assert!(
        matches!(
            refused,
            Err(ConsignmentError::Deposit(DepositBusinessRejection::EquipmentCommitted))
        ),
        "{refused:?}"
    );
    assert_eq!((sent.position().await, sent.listing().await), before);
    assert_eq!(sent.generations().await.len(), 1);
    assert_eq!(sent.count("consignment_withdrawals").await, 0);
}

/// 反例：Sale 已封闭、原件从未离库，但同合同的买家退款派发已起写而结果未知（无 Sent 绑定）。
/// 相反未知外写未清，撤回以 `custody_unproven` 拒绝，零痕迹。
#[tokio::test]
async fn postgres_sale_withdrawal_refuses_an_unknown_refund_dispatch() {
    let world = World::sold(Closure::CancelledUnpaid).await;
    succeeds(&world.owner, &unknown_refund_sql()).await;
    let before = (world.position().await, world.listing().await);

    let refused = world.deposit.request_withdrawal(withdraw(4, 6, 0x35)).await;

    assert!(
        matches!(
            refused,
            Err(ConsignmentError::Deposit(DepositBusinessRejection::CustodyUnproven))
        ),
        "{refused:?}"
    );
    assert_eq!((world.position().await, world.listing().await), before);
    assert_eq!(world.generations().await.len(), 1);
    assert_eq!(world.count("consignment_withdrawals").await, 0);
}

/// 同一卖家的活动挂单数（占额度的四种状态）。
async fn active_quota(world: &World) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM public.consignment_listings
         WHERE seller_user_id = $1
           AND state IN ('resolving', 'displayed', 'awaiting_deposit', 'available')",
    )
    .bind(id(USER_1))
    .fetch_one(&world.pool)
    .await
    .expect("活动额度")
}

/// D9 §11.1：自动重列关闭（夹具不走任何自动重列）、运营止售有效、卖家额度已满时，卖家仍可从已封闭的
/// 旧 Sale 直接撤回为 g+1 `withdrawal_only` 代；`available` 事件与活动额度变化均为零。反例：同样条件下
/// 重列被拒（重列需要额度且不得有运营止售），零痕迹。
///
/// 保护的不变量：2（锁内判定）、3（代、合同、事件、回执同提）、6（失败关闭）。
#[tokio::test]
async fn postgres_sale_withdrawal_ignores_an_effective_hold_and_a_full_quota() {
    let world = World::sold(Closure::CancelledUnpaid).await;
    // 额度为 1，且同一卖家已有另一件 resolving 挂单占满。
    let full = DepositRepository::without_reservation_crypto(
        world.deposit.store().clone(),
        DepositSettings::new(
            FeeBps::new(500).expect("费率"),
            ListingPrice::new(ListingPrice::MIN).expect("下限"),
            ListingPrice::new(ListingPrice::MAX).expect("上限"),
            24,
            1,
        ),
    )
    .with_dispatch_content(Arc::new(FakeContent));
    succeeds(
        &world.owner,
        &format!(
            "INSERT INTO public.consignment_listings
                (id, seller_user_id, world_id, external_equipment_id, mode, state, price,
                 pricing_version, fee_bps, state_version, service_epoch,
                 blacklist_count_at_register, blacklist_baseline_ids, created_at, updated_at,
                 expires_at)
             VALUES ('{OTHER_LISTING}', '{USER_1}', 'main', '43', 'consigned', 'resolving', 101, 1,
                     500, 1, 1, 0, ARRAY[]::uuid[], timestamptz '2026-01-01T04:00:00Z',
                     timestamptz '2026-01-01T04:00:00Z', timestamptz '2099-01-01T00:00:00Z');
             INSERT INTO public.consignment_commands
                (command_id, schema_version, actor_kind, actor_ref, operation, request_target,
                 request_digest, replay_key_id, listing_id, result_status, result_code,
                 result_body, created_at)
             VALUES ('{OTHER_REGISTER}', 1, 'public_user', '{USER_1}', 'register', 'new',
                     decode(repeat('17', 32), 'hex'), '{REPLAY}', '{OTHER_LISTING}', 202,
                     'accepted',
                     '{{\"command_id\":\"{OTHER_REGISTER}\",\"operation\":\"register\",
                        \"listing_id\":\"{OTHER_LISTING}\",\"listing_version\":1,
                        \"state\":\"resolving\",\"disposition\":\"registered\",
                        \"price\":101}}', timestamptz '2026-01-01T04:00:00Z');
             INSERT INTO public.consignment_events
                (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
                 to_state, reason_code, price, occurred_at)
             VALUES ('{OTHER_LISTING}', 1, '{OTHER_REGISTER}', 'public_user', 'registered', NULL,
                     'resolving', NULL, 101, timestamptz '2026-01-01T04:00:00Z');"
        ),
    )
    .await;
    // 有效运营止售：真实 Admin 写端建控制行（挂单版本 6 → 7）。
    let hold = full
        .set_operations_hold(SetOperationsHoldCommand {
            admin_actor: "fake-admin".to_owned(),
            command_id: command_id(0xb11),
            listing_id: id(LISTING),
            expected_version: ListingVersion::new(6).expect("版本"),
            request_hash: RequestHash::new([0x71; 32]),
            request_id: None,
            reason: ListingDelistReason::new("fake hold reason".to_owned()).expect("夹具说明"),
        })
        .await
        .expect("设运营止售");
    assert!(hold.hold_id.is_some(), "夹具必须有有效止售，否则本例不证明任何事");
    let quota_before = active_quota(&world).await;
    assert_eq!(quota_before, 1, "夹具必须占满额度");
    let available_before = world.listing_events("available").await;
    let commands_before = world.count("consignment_commands").await;

    // 反例：同样条件下重列被拒，不追加处分代、不改挂单。
    let refused = full
        .relist(RelistCommand {
            command_id: command_id(0xb12),
            seller_user_id: seller(),
            listing_id: id(LISTING),
            expected_version: ListingVersion::new(7).expect("版本"),
            request_hash: RequestHash::new([0x72; 32]),
            request_id: None,
        })
        .await;
    assert!(
        matches!(refused, Err(ConsignmentError::Deposit(DepositBusinessRejection::OperationsHold))),
        "{refused:?}"
    );
    assert_eq!(world.generations().await.len(), 1);
    assert_eq!(world.listing().await, ("held_off_market".to_owned(), 7));

    // 正例：直接撤回建 g+1 撤回代。
    let receipt = full.request_withdrawal(withdraw(0xb13, 7, 0x73)).await.expect("撤回");
    assert_eq!(
        (receipt.state, receipt.listing_version.value()),
        (ListingState::WithdrawalPending, 8)
    );
    assert_eq!(
        world.generations().await,
        vec![(1, "first_sale".to_owned(), None), (2, "withdrawal_only".to_owned(), Some(1))]
    );
    assert_eq!(world.listing_events("available").await, available_before);
    assert_eq!(active_quota(&world).await, quota_before);
    assert_eq!(world.count("consignment_sales").await, 1);
    assert_eq!(world.count("settlement_funding_entries").await, 0);
    // 拒绝回执与撤回回执各一行，没有其他命令。
    assert_eq!(world.count("consignment_commands").await, commands_before + 2);
}

/// Sale 封闭后的撤回（g+1 撤回代）同提撤回件首轮（PIWX）：经生产领取、衔接与登记尝试走到首次起写，
/// 得到 `FirstCommitted` 与恰一次 POST 许可，position 在 g+1 上 `in_custody → in_transit`。
#[tokio::test]
async fn postgres_sale_withdrawal_generation_dispatches_to_first_write() {
    let world = World::sold(Closure::CancelledUnpaid).await;
    world.deposit.request_withdrawal(withdraw(5, 6, 0x36)).await.expect("撤回");
    assert_eq!(world.position().await, ("in_custody".to_owned(), Some(2), 5));
    // 起写锁验 Worker 侧凭据命名空间与已提交的运行时 generation（与派发夹具同一准备）。
    succeeds(
        &world.owner,
        &format!(
            "UPDATE public.credential_keys SET scope = 'mooglemail:main' WHERE id = '{CREDENTIAL}';
             INSERT INTO public.mail_runtime_generation (scope, generation, activated_at)
             VALUES ('mail_cookie', 1, clock_timestamp()) ON CONFLICT (scope) DO NOTHING;"
        ),
    )
    .await;
    let store = world.deposit.store().clone();
    let now = Timestamp::now();
    let work = DispatchWorkRepository::new(store.clone(), Arc::new(FakeContent))
        .claim_dispatch_work(now, now + Duration::minutes(5))
        .await
        .expect("领取")
        .expect("撤回轮可领取");
    assert_eq!(work.frozen.template, "consignment_equipment_withdrawal");
    assert_eq!((work.frozen.cod, work.frozen.credits), (0, 0));
    let (delivery, token, fence) = store
        .test_lease_dispatch_round(
            work.obligation_id,
            work.claim_token,
            work.claim_fencing,
            dispatch_round_id(&work.key),
        )
        .await
        .expect("衔接")
        .expect("取得投递租约");
    let attempt = AttemptKey { delivery_id: delivery, fence };
    let service = DispatchService::new(Arc::new(store));
    service.start_attempt(work.key, attempt, token, work.frozen.clone()).await.expect("登记");
    let binding = DeliverySendCredentialBinding {
        credential_id: id(CREDENTIAL),
        credential_version: 1,
        runtime_generation: 1,
    };

    let started = service.mark_write_started(attempt, token, Some(binding)).await.expect("起写");

    assert!(matches!(started, WriteStart::FirstCommitted { .. }), "{started:?}");
    assert_eq!(world.position().await, ("in_transit".to_owned(), Some(2), 6));
    assert_eq!(world.count("delivery_send_attempts").await, 1, "恰一次 POST 许可");
}
