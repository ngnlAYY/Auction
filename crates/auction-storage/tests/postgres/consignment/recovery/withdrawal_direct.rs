//! 7C-2 撤回意愿兑现与无 Sale 直接撤回的 PG18 测试：取得事务同提建 Withdrawal、取得先到时旧版本过期、
//! 直接撤回不产生 `available` 事件且不碰名额与金额、并发撤回与购买至多一方获权。
//!
//! 夹具：main 世界已接受未取得的件（挂单版本 4）；取得后上架为 `available` 版本 5。购买侧用已合入的 6A
//! 写端（`PurchaseRepository`）。直接撤回的回执处置 `withdrawal_accepted` 与事件种类需要 000137 的共享替换段
//! （7M-2）放开账本与挂单事件约束，之前这些用例会被数据库拒绝。

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use auction_application::consignment::{
    ConsignmentError, CustodyControlStore, DepositBusinessRejection, ListingCommandOutcome,
    ListingCommandRejection, ListingCommandStore, PurchaseCommand, PurchaseOutcome, PurchaseStore,
    SellerListingTarget, UnrecordedListingRejection, WithdrawDepositCommand,
    WithdrawDepositReceipt, WithdrawListingCommand,
};
use auction_application::core::RequestHash;
use auction_application::settlement::{
    ConsignmentFundingPreflight, FundingFuture, FundingPreflightFact,
};
use auction_domain::consignment::funding::FundingMode;
use auction_domain::consignment::purchase::{PurchaseInput, PurchaseRejection};
use auction_domain::consignment::{
    ConsignmentSalePolicy, FeeBps, ListingPrice, ListingState, ListingVersion,
};
use auction_domain::{CommandId, SettlementWorld, UserId};
use auction_storage::{
    DatabaseStore, DepositRepository, DepositSettings, FirstDispatchContent,
    FirstDispatchContentSource, FirstDispatchRequest, PurchaseRepository, TestAfterLock,
    WithdrawalDispatchRequest,
};
use auction_time::Timestamp;
use sqlx::{AssertSqlSafe, PgPool};
use tokio::sync::Notify;
use tokio::time::sleep;
use uuid::Uuid;

use super::super::deposit::support::{
    ATTEMPT, DECISION, LISTING, POSITION, PROOF, PROOF_COMMAND, USER_1, USER_2, seed_accepted,
    succeeds,
};
use crate::support::TestDatabase;

const BOUNDARY_SOURCE: &str = "c7a00000-0000-4000-8000-000000000001";
const PRICE: i64 = 101;

/// 玩家 UUIDv4 命令 ID。
fn command_id(seed: u128) -> CommandId {
    CommandId::from_uuid(Uuid::from_u128(0x7c20_0000_0000_4000_8000_0000_0000_0000 + seed))
}

fn listing_id() -> Uuid {
    Uuid::parse_str(LISTING).expect("夹具挂单")
}

fn withdraw(seed: u128, version: u64, hash: u8) -> WithdrawDepositCommand {
    WithdrawDepositCommand {
        command_id: command_id(seed),
        seller_user_id: UserId::from_uuid(Uuid::parse_str(USER_1).expect("夹具卖家")),
        listing_id: listing_id(),
        expected_version: ListingVersion::new(version).expect("版本"),
        request_hash: RequestHash::new([hash; 32]),
        request_id: None,
    }
}

fn purchase(seed: u128) -> PurchaseCommand {
    PurchaseCommand {
        command_id: command_id(seed),
        request_hash: RequestHash::new([0x31; 32]),
        buyer_user_id: UserId::from_uuid(Uuid::parse_str(USER_2).expect("夹具买家")),
        input: PurchaseInput {
            listing_id: listing_id(),
            expected_version: ListingVersion::new(5).expect("版本"),
            expected_price: ListingPrice::new(PRICE).expect("价格"),
        },
        request_id: None,
    }
}

/// 资金预检恒 Ready：只服务购买侧的最小夹具。
struct ReadyPreflight;

impl ConsignmentFundingPreflight for ReadyPreflight {
    fn preflight(&self, _world: SettlementWorld) -> FundingFuture<'_, FundingPreflightFact> {
        Box::pin(async {
            Ok(FundingPreflightFact::Ready {
                uid: "7001".to_owned(),
                mode: FundingMode::StrictCash,
                boundary_at: Timestamp::parse_from_rfc3339("2026-01-02T00:00:00Z").expect("时刻"),
                boundary_source_id: Some(Uuid::parse_str(BOUNDARY_SOURCE).expect("夹具来源")),
                account_version: 0,
            })
        })
    }
}

/// 假首轮内容：明显的假文本与固定摘要。
struct FakeContent;

impl FirstDispatchContentSource for FakeContent {
    fn content(
        &self,
        request: &FirstDispatchRequest<'_>,
    ) -> Result<FirstDispatchContent, ConsignmentError> {
        Ok(FirstDispatchContent {
            template: "consignment_cod".to_owned(),
            subject: "fake subject".to_owned(),
            body_digest: [0xd1; 32],
            equipment_ids: vec![request.canonical_eid.to_owned()],
        })
    }

    fn withdrawal_content(
        &self,
        request: &WithdrawalDispatchRequest<'_>,
    ) -> Result<FirstDispatchContent, ConsignmentError> {
        Ok(FirstDispatchContent {
            template: "consignment_equipment_withdrawal".to_owned(),
            subject: "fake withdrawal subject".to_owned(),
            body_digest: [0xd2; 32],
            equipment_ids: vec![request.canonical_eid.to_owned()],
        })
    }
}

struct World {
    _database: TestDatabase,
    pool: PgPool,
    /// v1 本人命令入口（`ListingCommandStore`）所在的运行账号存储。
    store: DatabaseStore,
    deposit: DepositRepository,
    purchases: PurchaseRepository,
}

impl World {
    /// main 件已接受、尚未取得；寄售闸门打开，购买侧资金账户就绪。
    async fn accepted() -> Self {
        let database = TestDatabase::create().await.expect("克隆模板库");
        let runtime = database.store().await.expect("运行账号");
        let pool = runtime.postgres_pool().clone();
        seed_accepted(&pool).await;
        let owner = database.maintenance_store().await.expect("维护账号");
        succeeds(
            owner.postgres_pool(),
            &format!(
                "UPDATE public.consignment_lookup_control
                 SET admitting = true, enabled_epoch = 1,
                     worker_ready_until = clock_timestamp() + interval '1 hour' WHERE id = 1;
                 INSERT INTO public.settlement_funding_accounts (uid, status)
                 VALUES ('7001', 'ready') ON CONFLICT DO NOTHING;
                 INSERT INTO public.settlement_funding_sources
                    (id, uid, kind, source_key, amount, proof_digest)
                 VALUES ('{BOUNDARY_SOURCE}', '7001', 'reconciliation',
                         'c7a00000-0000-4000-8000-0000000000f1', 0,
                         decode(repeat('a1', 32), 'hex'));"
            ),
        )
        .await;
        let deposit = DepositRepository::without_reservation_crypto(
            runtime.clone(),
            DepositSettings::new(
                FeeBps::new(500).expect("费率"),
                ListingPrice::new(ListingPrice::MIN).expect("下限"),
                ListingPrice::new(ListingPrice::MAX).expect("上限"),
                24,
                100,
            ),
        )
        .with_dispatch_content(Arc::new(FakeContent));
        let purchases = PurchaseRepository::new(
            runtime.clone(),
            Arc::new(ReadyPreflight),
            Arc::new(FakeContent),
            ConsignmentSalePolicy::new(7, 3, 5).expect("策略"),
        );
        Self { _database: database, pool, store: runtime, deposit, purchases }
    }

    /// 整封精确 Take 的取得事务。
    async fn take(&self) {
        self.deposit
            .test_apply_take(
                Uuid::parse_str(PROOF_COMMAND).expect("证明命令"),
                Uuid::parse_str(DECISION).expect("决定"),
                Uuid::parse_str(PROOF).expect("证明"),
                Uuid::parse_str(ATTEMPT).expect("attempt"),
                listing_id(),
            )
            .await
            .expect("取得");
    }

    async fn count(&self, table: &str) -> i64 {
        sqlx::query_scalar(AssertSqlSafe(format!("SELECT count(*) FROM public.{table}")))
            .fetch_one(&self.pool)
            .await
            .expect("计数")
    }

    async fn listing(&self) -> (String, i64) {
        sqlx::query_as("SELECT state, state_version FROM public.consignment_listings WHERE id = $1")
            .bind(listing_id())
            .fetch_one(&self.pool)
            .await
            .expect("读取挂单")
    }

    /// 挂单事件按版本排序的 (版本, 种类)。
    async fn events(&self) -> Vec<(i64, String)> {
        sqlx::query_as(
            "SELECT state_version, event_kind FROM public.consignment_events
             WHERE listing_id = $1 ORDER BY state_version",
        )
        .bind(listing_id())
        .fetch_all(&self.pool)
        .await
        .expect("读取事件")
    }

    async fn contract_kinds(&self) -> Vec<String> {
        sqlx::query_scalar("SELECT kind FROM public.consignment_contracts ORDER BY created_at")
            .fetch_all(&self.pool)
            .await
            .expect("读取合同")
    }
}

/// 意愿先到：取得事务同提建 Withdrawal，挂单直接进入 `withdrawal_pending`，从未出现 `available`。
///
/// 不变量 2、3：合同、返还义务、挂单投影与取得回执同事务；返还义务金额为 0。
#[tokio::test]
async fn postgres_withdrawal_intent_first_take_builds_withdrawal_in_the_take_transaction() {
    let world = World::accepted().await;
    let intent = world.deposit.request_withdrawal(withdraw(1, 4, 0x11)).await.expect("登记意愿");
    assert_eq!(intent.disposition.as_str(), "withdrawal_requested");
    assert_eq!(world.count("consignment_contracts").await, 0, "意愿阶段尚无合同");

    world.take().await;

    assert_eq!(world.contract_kinds().await, vec!["withdrawal".to_owned()]);
    let (generation, origin, intent_id): (i64, Uuid, Option<Uuid>) = sqlx::query_as(
        "SELECT c.generation, c.origin_command_id, w.intent_id
         FROM public.consignment_contracts c
         JOIN public.consignment_withdrawals w ON w.contract_id = c.contract_id",
    )
    .fetch_one(&world.pool)
    .await
    .expect("读取 Withdrawal");
    assert_eq!((generation, origin), (1, command_id(1).as_uuid()));
    assert_eq!(intent_id, intent.intent_id);
    let obligation: (String, i64) = sqlx::query_as(
        "SELECT obligation_kind, amount FROM public.settlement_obligations
         WHERE contract_id IS NOT NULL",
    )
    .fetch_one(&world.pool)
    .await
    .expect("返还义务");
    assert_eq!(obligation, ("withdrawn_return".to_owned(), 0));
    assert_eq!(world.count("consignment_sales").await, 0);
    assert_eq!(world.listing().await, ("withdrawal_pending".to_owned(), 6));
    assert!(world.events().await.iter().all(|(_, kind)| kind != "available"));
}

/// 取得先到：挂单已上架后，持取得前版本的撤回请求得 `stale_version`，不建合同也不留意愿。
#[tokio::test]
async fn postgres_withdrawal_take_first_rejects_the_old_version_request() {
    let world = World::accepted().await;
    world.take().await;
    assert_eq!(world.listing().await, ("available".to_owned(), 5));
    assert_eq!(world.count("consignment_contracts").await, 0, "无意愿的取得不建合同");

    let stale = world.deposit.request_withdrawal(withdraw(2, 4, 0x12)).await;

    assert!(
        matches!(stale, Err(ConsignmentError::Deposit(DepositBusinessRejection::StaleVersion))),
        "{stale:?}"
    );
    assert_eq!(world.count("consignment_contracts").await, 0);
    assert_eq!(world.count("consignment_custody_controls").await, 0);
    assert_eq!(world.listing().await, ("available".to_owned(), 5));
}

/// 直接撤回：`available` 版本 5 直达 `withdrawal_pending` 版本 6，事件序列没有第二个 `available`；
/// 返还义务零金额、没有 Sale；同命令重放得原回执且不增行，同 ID 异请求冲突。
///
/// 不变量 1、2、3：回执、合同、挂单投影与事件同事务；占用名额的活动态只在撤回后减少，不经过任何新的活动态。
#[tokio::test]
async fn postgres_withdrawal_direct_skips_available_and_leaves_quota_and_money_untouched() {
    let world = World::accepted().await;
    world.take().await;
    let command = withdraw(3, 5, 0x13);

    let receipt = world.deposit.request_withdrawal(command.clone()).await.expect("直接撤回");

    assert_eq!(receipt.disposition.as_str(), "withdrawal_accepted");
    assert_eq!(
        (receipt.state, receipt.listing_version.value()),
        (ListingState::WithdrawalPending, 6)
    );
    assert!(receipt.intent_id.is_none() && receipt.contract_id.is_some());
    assert_eq!(world.contract_kinds().await, vec!["withdrawal".to_owned()]);
    let events = world.events().await;
    assert_eq!(
        events.iter().filter(|(_, kind)| kind == "available").count(),
        1,
        "只有取得时那一次"
    );
    assert_eq!(events.last(), Some(&(6, "withdrawal_accepted".to_owned())));
    assert_eq!(events.len(), 6, "5 之后只多 6 一个版本：不经过 available 的任何重新上架");
    let active_quota: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.consignment_listings
         WHERE seller_user_id = $1
           AND state IN ('resolving', 'displayed', 'awaiting_deposit', 'available')",
    )
    .bind(Uuid::parse_str(USER_1).expect("夹具卖家"))
    .fetch_one(&world.pool)
    .await
    .expect("活动名额");
    assert_eq!(active_quota, 0, "撤回后不占活动名额");
    let amount: i64 = sqlx::query_scalar(
        "SELECT amount FROM public.settlement_obligations WHERE contract_id IS NOT NULL",
    )
    .fetch_one(&world.pool)
    .await
    .expect("返还义务");
    assert_eq!(amount, 0);
    assert_eq!(world.count("consignment_sales").await, 0);
    let position: (String, Option<i64>) = sqlx::query_as(
        "SELECT state, current_generation FROM public.consignment_custody_positions WHERE id = $1",
    )
    .bind(Uuid::parse_str(POSITION).expect("夹具 position"))
    .fetch_one(&world.pool)
    .await
    .expect("读取 position");
    assert_eq!(position, ("in_custody".to_owned(), Some(1)), "原代上直接绑定，不追加新代");

    let replay: WithdrawDepositReceipt =
        world.deposit.request_withdrawal(command).await.expect("同命令重放");
    assert_eq!(replay, receipt);
    assert_eq!(world.count("consignment_contracts").await, 1);
    assert_eq!(world.listing().await, ("withdrawal_pending".to_owned(), 6));
    let conflict = world.deposit.request_withdrawal(withdraw(3, 5, 0x14)).await;
    assert!(matches!(conflict, Err(ConsignmentError::CommandIdConflict)), "{conflict:?}");
    // 已接受的直接撤回不再接受第二条撤回命令：不造第二份合同、意愿或空版本。
    let again = world.deposit.request_withdrawal(withdraw(4, 6, 0x15)).await;
    assert!(
        matches!(again, Err(ConsignmentError::Deposit(DepositBusinessRejection::ActionNotAllowed))),
        "{again:?}"
    );
    assert_eq!(world.count("consignment_contracts").await, 1);
    assert_eq!(world.count("consignment_custody_controls").await, 0);
    assert_eq!(world.listing().await, ("withdrawal_pending".to_owned(), 6));
}

/// 同一 v1 撤回命令；`hash` 填满请求摘要。
fn withdraw_v1(seed: u128, version: u64, hash: u8) -> WithdrawListingCommand {
    WithdrawListingCommand {
        request_id: None,
        request_hash: RequestHash::new([hash; 32]),
        target: SellerListingTarget {
            command_id: command_id(seed),
            listing_id: listing_id(),
            seller_user_id: UserId::from_uuid(Uuid::parse_str(USER_1).expect("夹具卖家")),
            expected_version: ListingVersion::new(version).expect("版本"),
        },
    }
}

/// 同一 `command_id` 不跨 schema 版本（8B-b）：v2 撤回已提交、浏览器的结果未知，旧壳以同一 ID
/// 走 v1 入口重试时得到冲突，不被当成新命令、也不把 v2 回执当 v1 回执重放，零新增写入；v2 入口
/// 仍重放原回执。反方向同理：v1 已落账的 ID 在 v2 入口冲突。
///
/// 不变量 1：同 ID 同请求返回原结果，同 ID 不同请求（含不同 schema 版本）冲突。
#[tokio::test]
async fn postgres_consignment_command_id_does_not_cross_schema_versions() {
    let world = World::accepted().await;
    world.take().await;
    let v2 = withdraw(20, 5, 0x31);
    let receipt = world.deposit.request_withdrawal(v2.clone()).await.expect("v2 直接撤回");
    let before = (
        world.count("consignment_commands").await,
        world.events().await,
        world.listing().await,
        world.count("consignment_contracts").await,
    );

    // 反例：旧壳以同一 ID 走 v1 入口重试。v1 与 v2 的规范编码由应用层分域，摘要必然不同。
    let crossed = world.store.withdraw(withdraw_v1(20, 6, 0x41)).await;
    assert!(
        matches!(
            crossed,
            Ok(ListingCommandOutcome::Rejected(ListingCommandRejection::Unrecorded(
                UnrecordedListingRejection::CommandIdConflict
            )))
        ),
        "{crossed:?}"
    );
    let after = (
        world.count("consignment_commands").await,
        world.events().await,
        world.listing().await,
        world.count("consignment_contracts").await,
    );
    assert_eq!(after, before, "跨版本重试不得写入任何东西");
    // 正例：v2 入口同命令仍重放原回执。
    let replay = world.deposit.request_withdrawal(v2).await.expect("v2 重放");
    assert_eq!(replay, receipt);

    // 反方向：v1 入口先落账（撤回已不可用，回执是业务拒绝），v2 入口同 ID 冲突。
    world.store.withdraw(withdraw_v1(21, 6, 0x32)).await.expect("v1 入口落账");
    let v1_rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.consignment_commands
         WHERE command_id = $1 AND schema_version = 1",
    )
    .bind(command_id(21).as_uuid())
    .fetch_one(&world.pool)
    .await
    .expect("读取 v1 账本行");
    assert_eq!(v1_rows, 1, "夹具必须先有 v1 账本行，否则本例不证明任何事");
    let reverse = world.deposit.request_withdrawal(withdraw(21, 6, 0x32)).await;
    assert!(matches!(reverse, Err(ConsignmentError::CommandIdConflict)), "{reverse:?}");
    assert_eq!(world.listing().await, ("withdrawal_pending".to_owned(), 6));
}

/// 取锁后停住、直到 `release` 被通知才继续的钩子；`entered` 在停住时通知。
fn pause_hook(entered: Arc<Notify>, release: Arc<Notify>) -> Box<TestAfterLock> {
    Box::new(move || -> Pin<Box<dyn Future<Output = ()> + Send>> {
        let (entered, release) = (entered.clone(), release.clone());
        Box::pin(async move {
            entered.notify_one();
            release.notified().await;
        })
    })
}

/// 并发撤回与购买至多一方获权：购买先持锁则撤回被挡住、提交后被拒，只有 Sale；
/// 撤回先提交则购买得 `listing_not_active`，只有 Withdrawal，没有任何成交事实。
///
/// 不变量 2：同一实物锁下串行，`(position, 代)` 合同唯一约束兜底。
#[tokio::test]
async fn postgres_withdrawal_direct_and_purchase_race_has_exactly_one_winner() {
    // 顺序一：购买先持锁。
    let world = World::accepted().await;
    world.take().await;
    let (entered, release) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
    let buying = {
        let (purchases, entered, release) =
            (world.purchases.clone(), entered.clone(), release.clone());
        tokio::spawn(async move {
            let hook = pause_hook(entered, release);
            purchases.test_purchase_hooked(purchase(10), Some(&*hook)).await
        })
    };
    entered.notified().await;
    let withdrawing = {
        let deposit = world.deposit.clone();
        tokio::spawn(async move { deposit.request_withdrawal(withdraw(11, 5, 0x21)).await })
    };
    sleep(Duration::from_millis(250)).await;
    assert!(!withdrawing.is_finished(), "撤回应被购买持有的实物锁挡住");
    release.notify_one();
    let bought = buying.await.expect("购买任务").expect("购买");
    assert!(matches!(bought, PurchaseOutcome::Purchased(_)), "{bought:?}");
    let withdrawn = withdrawing.await.expect("撤回任务");
    assert!(withdrawn.is_err(), "购买已提交，撤回不得获权：{withdrawn:?}");
    assert_eq!(world.contract_kinds().await, vec!["sale".to_owned()]);
    assert_eq!(world.count("consignment_withdrawals").await, 0);

    // 顺序二：撤回先提交。
    let world = World::accepted().await;
    world.take().await;
    world.deposit.request_withdrawal(withdraw(12, 5, 0x22)).await.expect("撤回");
    let blocked = world.purchases.purchase(purchase(13)).await.expect("购买");
    assert!(
        matches!(blocked, PurchaseOutcome::Rejected(PurchaseRejection::ListingNotActive)),
        "{blocked:?}"
    );
    assert_eq!(world.contract_kinds().await, vec!["withdrawal".to_owned()]);
    assert_eq!(world.count("consignment_sales").await, 0);
    assert_eq!(world.count("settlement_obligations").await, 1, "只有返还义务，没有付款或卖款义务");
}
