//! v2 本人 / Admin 读模型：代管态、处分代、有效止售，以及允许动作必须是服务端可执行动作的子集。
//!
//! 保护的不变量：6（不宣告不可执行的动作，非法状态失败关闭）与 7（本人读只按会话用户过滤，
//! 非本人与不存在同样返回空）。动作清单只来自领域 `seller_actions` / `admin_actions`。

use auction_application::consignment::{
    AdminListingQuery, AdminListingReadV2, ConsignmentError, CustodyControlStore,
    DepositBusinessRejection, ListingBusinessRejection, ListingCommandStore, ListingDelistReason,
    ListingPageWindow, OwnedListingReadV2, PrivateListingQuery, PrivateListingV2,
    PrivateListingViewV2, ReleaseOperationsHoldCommand, RepriceListingCommand, ReservationStore,
    SellerListingTarget, SetOperationsHoldCommand, WithdrawDepositCommand,
};
use auction_domain::consignment::deposit::{AdminAction, PositionState, SellerAction};
use auction_domain::consignment::{ListingPrice, ListingState};
use auction_domain::{SettlementWorld, UserId};
use auction_storage::{DepositRepository, DepositSettings};
use uuid::Uuid;

use super::super::support::{self, ATTEMPT, DECISION, PROOF, PROOF_COMMAND};
use super::helpers::{self, client_command, hash, issue_command, repository, seed_mail_ready};
use crate::support::TestDatabase;

/// 夹具所处的状态；每种都经真实命令或种子到达，不手改状态列。
#[derive(Clone, Copy, Debug)]
enum Shape {
    AwaitingDeposit,
    DepositAccepted,
    Available,
    HeldByOperations,
    WithdrawalPending,
    Closed,
}

const SHAPES: [Shape; 6] = [
    Shape::AwaitingDeposit,
    Shape::DepositAccepted,
    Shape::Available,
    Shape::HeldByOperations,
    Shape::WithdrawalPending,
    Shape::Closed,
];

struct Fixture {
    database: TestDatabase,
    repository: DepositRepository,
    listing: Uuid,
    seller: UserId,
}

fn seller() -> UserId {
    UserId::from_uuid(Uuid::parse_str(support::USER_1).expect("固定卖家"))
}

fn query() -> PrivateListingQuery {
    PrivateListingQuery {
        window: ListingPageWindow::new(Some(1), Some(20)).expect("有效分页"),
        world: None,
        mode: None,
        state: None,
    }
}

fn gate_rejection(error: &ConsignmentError) -> bool {
    matches!(
        error,
        ConsignmentError::Deposit(
            DepositBusinessRejection::ActionNotAllowed | DepositBusinessRejection::ListingNotActive
        ) | ConsignmentError::Business(ListingBusinessRejection::ListingNotActive)
    )
}

async fn current(fixture: &Fixture) -> PrivateListingV2 {
    fixture
        .repository
        .owned_detail_v2(fixture.seller, fixture.listing)
        .await
        .expect("v2 本人详情")
        .expect("本人可见")
}

async fn withdraw(fixture: &Fixture, seed: u128) -> Result<(), ConsignmentError> {
    let version = current(fixture).await.version;
    fixture
        .repository
        .request_withdrawal(WithdrawDepositCommand {
            command_id: client_command(seed),
            seller_user_id: fixture.seller,
            listing_id: fixture.listing,
            expected_version: version,
            request_hash: hash(0x90),
            request_id: None,
        })
        .await
        .map(drop)
}

async fn hold(fixture: &Fixture, seed: u128) -> Result<(), ConsignmentError> {
    let version = current(fixture).await.version;
    fixture
        .repository
        .set_operations_hold(SetOperationsHoldCommand {
            admin_actor: "fake-admin".to_owned(),
            command_id: client_command(seed),
            listing_id: fixture.listing,
            expected_version: version,
            request_hash: hash(0x91),
            request_id: None,
            reason: ListingDelistReason::new("fixture hold".to_owned()).expect("说明"),
        })
        .await
        .map(drop)
}

async fn take(fixture: &Fixture) {
    // 取得前打开寄售闸门，否则取得决策会因服务停用落入 held_off_market。
    sqlx::query(
        "UPDATE public.consignment_lookup_control
         SET admitting = true, enabled_epoch = 1,
             worker_ready_until = clock_timestamp() + interval '1 hour'
         WHERE id = 1",
    )
    .execute(fixture.database.store().await.expect("运行账号").postgres_pool())
    .await
    .expect("打开寄售闸门");
    fixture
        .repository
        .test_apply_take(
            Uuid::parse_str(PROOF_COMMAND).expect("证明命令"),
            Uuid::parse_str(DECISION).expect("决定"),
            Uuid::parse_str(PROOF).expect("证明"),
            Uuid::parse_str(ATTEMPT).expect("attempt"),
            fixture.listing,
        )
        .await
        .expect("取得");
}

async fn fixture(shape: Shape) -> Fixture {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let repository = repository(&database).await;
    let listing = match shape {
        Shape::AwaitingDeposit | Shape::Closed => {
            let listing = Uuid::parse_str(helpers::LISTING).expect("固定挂单");
            let maintenance = database.maintenance_store().await.expect("维护连接");
            helpers::seed_listing(maintenance.postgres_pool(), listing, SettlementWorld::Main)
                .await;
            seed_mail_ready(maintenance.postgres_pool()).await;
            listing
        }
        _ => {
            let runtime = database.store().await.expect("运行账号");
            support::seed_accepted(runtime.postgres_pool()).await;
            Uuid::parse_str(support::LISTING).expect("固定挂单")
        }
    };
    let fixture = Fixture { database, repository, listing, seller: seller() };
    match shape {
        Shape::AwaitingDeposit | Shape::DepositAccepted => {}
        Shape::Available => take(&fixture).await,
        Shape::HeldByOperations => hold(&fixture, 700).await.expect("设止售"),
        Shape::WithdrawalPending => {
            withdraw(&fixture, 701).await.expect("登记撤回意愿");
            take(&fixture).await;
        }
        Shape::Closed => withdraw(&fixture, 702).await.expect("关闭未代管件"),
    }
    fixture
}

fn expected(shape: Shape) -> (Vec<SellerAction>, Vec<AdminAction>) {
    use SellerAction::{IssueReservation, Reprice, Withdraw};
    match shape {
        Shape::AwaitingDeposit => {
            (vec![Reprice, Withdraw, IssueReservation], vec![AdminAction::Delist])
        }
        Shape::DepositAccepted | Shape::Available => (vec![Withdraw], vec![AdminAction::Delist]),
        Shape::HeldByOperations => (vec![Withdraw], vec![AdminAction::ReleaseHold]),
        Shape::WithdrawalPending | Shape::Closed => (Vec::new(), Vec::new()),
    }
}

async fn run_seller(fixture: &Fixture, action: SellerAction) -> Result<(), ConsignmentError> {
    let version = current(fixture).await.version;
    match action {
        SellerAction::Withdraw => withdraw(fixture, 800).await,
        SellerAction::IssueReservation => fixture
            .repository
            .issue_reservation(issue_command(
                client_command(801),
                fixture.listing,
                version.value(),
                0x92,
            ))
            .await
            .map(drop),
        SellerAction::Reprice => fixture
            .repository
            .store()
            .reprice(RepriceListingCommand {
                request_hash: hash(0x93),
                target: SellerListingTarget {
                    command_id: client_command(802),
                    seller_user_id: fixture.seller,
                    listing_id: fixture.listing,
                    expected_version: version,
                },
                request_id: None,
                price: ListingPrice::new(150).expect("标价"),
                enabled: true,
            })
            .await?
            .into_result()
            .map(drop),
        SellerAction::Relist => panic!("第 5 期不得宣告重列"),
    }
}

async fn run_admin(fixture: &Fixture, action: AdminAction) -> Result<(), ConsignmentError> {
    match action {
        AdminAction::Delist => hold(fixture, 803).await,
        AdminAction::ReleaseHold => {
            let listing = current(fixture).await;
            let hold = listing.custody.hold.expect("止售态必有有效止售");
            fixture
                .repository
                .release_operations_hold(ReleaseOperationsHoldCommand {
                    admin_actor: "fake-admin".to_owned(),
                    command_id: client_command(804),
                    listing_id: fixture.listing,
                    expected_version: listing.version,
                    hold_id: hold.hold_id,
                    request_hash: hash(0x94),
                    request_id: None,
                })
                .await
                .map(drop)
        }
    }
}

/// 核心：下发的每个动作对应命令都必须返回 `Ok`；
/// 没有动作的终态则命令确实被这两类拒绝，证明“空”不是漏报。
#[tokio::test]
async fn postgres_v2_allowed_actions_are_executable_for_every_state() {
    for shape in SHAPES {
        let base = fixture(shape).await;
        let record = current(&base).await;
        let (seller_expected, admin_expected) = expected(shape);
        assert_eq!(record.seller_actions(), seller_expected, "{shape:?} 本人动作");
        let admin = base
            .repository
            .admin_detail_v2(base.listing)
            .await
            .expect("Admin 详情")
            .expect("Admin 可见");
        assert_eq!(admin.record.listing.admin_actions(), admin_expected, "{shape:?} Admin 动作");

        for action in seller_expected {
            let world = fixture(shape).await;
            if let Err(error) = run_seller(&world, action).await {
                panic!("{shape:?} 下发的本人动作 {action:?} 必须执行成功：{error:?}");
            }
        }
        for action in admin_expected {
            let world = fixture(shape).await;
            if let Err(error) = run_admin(&world, action).await {
                panic!("{shape:?} 下发的 Admin 动作 {action:?} 必须执行成功：{error:?}");
            }
        }
    }

    // 反例：已关闭的挂单没有任何动作，对应命令也确实被拒绝。
    let closed = fixture(Shape::Closed).await;
    let error = withdraw(&closed, 810).await.expect_err("已关闭不得撤回");
    assert!(gate_rejection(&error), "{error:?}");
    let error = hold(&closed, 811).await.expect_err("已关闭不得设止售");
    assert!(gate_rejection(&error), "{error:?}");
}

/// 撤回意愿登记后不再宣告撤回；止售生效后 Admin 只能解除，不能再设止售。
#[tokio::test]
async fn postgres_v2_actions_follow_intent_and_hold_facts() {
    let fixture = fixture(Shape::DepositAccepted).await;
    assert_eq!(current(&fixture).await.seller_actions(), vec![SellerAction::Withdraw]);
    withdraw(&fixture, 820).await.expect("登记撤回意愿");
    let intent = current(&fixture).await;
    assert!(intent.custody.withdrawal_intent_registered);
    assert!(intent.seller_actions().is_empty(), "已登记意愿后不再宣告撤回");
    assert_eq!(intent.admin_actions(), vec![AdminAction::Delist]);

    hold(&fixture, 821).await.expect("设止售");
    let held = current(&fixture).await;
    assert_eq!(held.view.effective_state(), ListingState::HeldOffMarket);
    assert_eq!(held.admin_actions(), vec![AdminAction::ReleaseHold]);
    let error = hold(&fixture, 822).await.expect_err("已有有效止售");
    assert!(gate_rejection(&error), "{error:?}");
}

/// 本人只看自己的挂单；非本人与不存在不可区分；Admin 看全部并带卖家与止售。
#[tokio::test]
async fn postgres_v2_reads_are_owner_scoped_and_admin_sees_the_seller_and_hold() {
    let fixture = fixture(Shape::HeldByOperations).await;
    let owned = fixture.repository.list_owned_v2(fixture.seller, query()).await.expect("本人列表");
    assert_eq!((owned.total, owned.items.len()), (1, 1));
    let record = &owned.items[0];
    assert!(matches!(
        record.view,
        PrivateListingViewV2::HeldOffMarket { reason, .. }
            if reason == auction_domain::consignment::HeldOffMarketReason::OperationsHold
    ));
    // 对尚未取得的已接受件设止售：没有处分代，position 仍在等待取得。
    assert_eq!(record.generation(), None);
    assert_eq!(
        record.custody.position.map(|position| position.state),
        Some(PositionState::AwaitingTake)
    );
    let hold = record.custody.hold.expect("有效止售");

    let stranger = UserId::from_uuid(Uuid::new_v4());
    let other = fixture.repository.list_owned_v2(stranger, query()).await.expect("他人列表");
    assert_eq!((other.total, other.items.len()), (0, 0));
    assert!(
        fixture
            .repository
            .owned_detail_v2(stranger, fixture.listing)
            .await
            .expect("他人详情")
            .is_none()
    );
    assert!(
        fixture
            .repository
            .owned_detail_v2(fixture.seller, Uuid::new_v4())
            .await
            .expect("不存在")
            .is_none()
    );

    let admin = fixture
        .repository
        .list_admin_v2(AdminListingQuery { listings: query(), seller_user_id: None })
        .await
        .expect("Admin 列表");
    assert_eq!((admin.total, admin.items.len()), (1, 1));
    assert_eq!(admin.items[0].seller_user_id, fixture.seller);
    assert_eq!(admin.items[0].listing.custody.hold.map(|found| found.hold_id), Some(hold.hold_id));
    // 本人与 Admin 各看各的动作：卖家只能撤回，运营只能解除止售。
    assert_eq!(record.seller_actions(), vec![SellerAction::Withdraw]);
    assert_eq!(admin.items[0].listing.admin_actions(), vec![AdminAction::ReleaseHold]);
    let filtered = fixture
        .repository
        .list_admin_v2(AdminListingQuery { listings: query(), seller_user_id: Some(stranger) })
        .await
        .expect("按他人筛选");
    assert_eq!(filtered.total, 0);
}

/// D9 §7：寄售停用后，停用装配用的只读端口（无预约码密码实现）仍能读本人与 Admin 的 v2 历史。
#[tokio::test]
async fn postgres_v2_reads_survive_disable_with_the_read_only_repository() {
    let fixture = fixture(Shape::Available).await;
    sqlx::query("UPDATE public.consignment_lookup_control SET admitting = false WHERE id = 1")
        .execute(fixture.database.store().await.expect("运行账号").postgres_pool())
        .await
        .expect("停用寄售");
    let settings = DepositSettings::with_standard_price_bounds(
        auction_domain::consignment::FeeBps::new(500).expect("测试费率"),
        24,
        100,
    )
    .expect("收件设置");
    let reader = DepositRepository::without_reservation_crypto(
        fixture.database.store().await.expect("运行账号"),
        settings,
    );
    let owned = reader.list_owned_v2(fixture.seller, query()).await.expect("停用后本人历史可读");
    assert_eq!((owned.total, owned.items.len()), (1, 1));
    assert_eq!(owned.items[0].view.effective_state(), ListingState::Closed);
    let detail = reader.owned_detail_v2(fixture.seller, fixture.listing).await.expect("本人详情");
    assert!(detail.is_some());
    let admin = reader.admin_detail_v2(fixture.listing).await.expect("Admin 详情");
    assert!(admin.is_some());
}

/// 有效态筛选与总数同一过滤；强制终结（停用）把 available 读成关闭而不是整页失败。
#[tokio::test]
async fn postgres_v2_state_filter_and_forced_closure_stay_readable() {
    let fixture = fixture(Shape::Available).await;
    let mut by_state = query();
    by_state.state = Some(ListingState::Available);
    let page = fixture.repository.list_owned_v2(fixture.seller, by_state).await.expect("按态");
    assert_eq!((page.total, page.items.len()), (1, 1));
    assert_eq!(page.items[0].generation(), Some(1));
    by_state.state = Some(ListingState::HeldOffMarket);
    let page = fixture.repository.list_owned_v2(fixture.seller, by_state).await.expect("按态");
    assert_eq!((page.total, page.items.len()), (0, 0));

    sqlx::query("UPDATE public.consignment_lookup_control SET admitting = false WHERE id = 1")
        .execute(fixture.database.store().await.expect("运行账号").postgres_pool())
        .await
        .expect("停用寄售");
    let record = current(&fixture).await;
    assert_eq!(record.view.effective_state(), ListingState::Closed);
    assert_eq!(record.view.effective_reason_code(), Some("service_disabled"));
    assert_eq!(record.view.stored_state(), ListingState::Available);
    assert!(record.seller_actions().is_empty() && record.admin_actions().is_empty());
}
