//! 竞价增量 v1/v2、代理反超与上限出价、软截止的纯逻辑回归。
//!
//! 规划器不访问数据库或网络；测试直接构造锁内快照和代理授权，验证公开事实金额、
//! 代理终态与最终 lot 投影。所有金额保持信用网格；历史非整千当前价只经显式入口进入。

use auction_domain::{
    AuctionRules, BidAmount, BidId, BidMechanism, CommandId, CompetitionPlan, CompetitionTrigger,
    DEFAULT_START_PRICE, DomainError, LotBidState, LotId, MAX_BID_AMOUNT, PROXY_LAST_INCREMENT,
    ProxyAuthorization, ProxyAuthorizationId, ProxyMode, ProxyStatus, ProxyTerminalReason,
    ProxyTransitionKind, UserId,
};
use chrono::{DateTime, Utc};

fn must<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
    result.unwrap_or_else(|error| panic!("测试夹具必须合法: {error:?}"))
}

fn required<T>(value: Option<T>) -> T {
    value.unwrap_or_else(|| panic!("测试夹具必须存在"))
}

fn at(hour: u32, minute: u32, second: u32) -> DateTime<Utc> {
    required(chrono::TimeZone::with_ymd_and_hms(&Utc, 2026, 1, 1, hour, minute, second).single())
}

fn amount(value: i64) -> BidAmount {
    must(BidAmount::new(value))
}

fn rules(version: u32) -> AuctionRules {
    AuctionRules { bidding_version: version, ..AuctionRules::default() }
}

fn lot_state(
    lot_id: LotId,
    current_price: Option<BidAmount>,
    leader_id: Option<UserId>,
    end: DateTime<Utc>,
) -> LotBidState {
    LotBidState {
        lot_id,
        start_price: amount(DEFAULT_START_PRICE),
        current_price,
        leader_id,
        leader_proxy_last: false,
        material: true,
        effective_end_at: end,
        state_version: 1,
        last_proxy_sequence: 1,
        slow_generation: 0,
        slow_authorization_id: None,
        slow_due_at: None,
    }
}

fn active_proxy(
    lot_id: LotId,
    user_id: UserId,
    maximum: i64,
    mode: ProxyMode,
) -> ProxyAuthorization {
    ProxyAuthorization::new(
        ProxyAuthorizationId::new(),
        lot_id,
        user_id,
        CommandId::new(),
        1,
        1,
        amount(maximum),
        mode,
    )
    .unwrap_or_else(|error| panic!("测试代理授权必须合法: {error:?}"))
}

fn ordinary_bid(
    actor: UserId,
    value: BidAmount,
    now: DateTime<Utc>,
    bid_count: usize,
) -> CompetitionTrigger {
    CompetitionTrigger::OrdinaryBid {
        command_id: CommandId::new(),
        actor_id: actor,
        amount: value,
        bid_ids: (0..bid_count).map(|_| BidId::new()).collect(),
        now,
    }
}

#[test]
fn increments_follow_frozen_v1_and_v2_rules() {
    let v2 = rules(2);
    assert_eq!(v2.next_minimum_bid(None, None, true, false).unwrap().value(), DEFAULT_START_PRICE);
    let current = amount(100_000);
    assert_eq!(v2.next_minimum_bid(None, Some(current), true, false).unwrap().value(), 105_000);
    // v2 的统一最低价不因自动代理身份改变，代理也必须严格反超。
    assert_eq!(v2.next_minimum_bid(None, Some(current), true, true).unwrap().value(), 105_000);

    // 旧版 1111 的非整千当前价按信用网格向上折算后加回原值，得到 2111。
    let legacy_price = BidAmount::from_persisted_positive(1_111).unwrap();
    assert_eq!(v2.next_minimum_bid(None, Some(legacy_price), true, false).unwrap().value(), 2_111);

    // 只有冻结的 v1 自动代理保留固定 1000 增量。
    let v1 = rules(1);
    assert_eq!(
        v1.next_minimum_bid(None, Some(amount(105_000)), true, true).unwrap().value(),
        105_000 + PROXY_LAST_INCREMENT
    );
    assert_eq!(v1.next_minimum_bid(None, Some(current), true, false).unwrap().value(), 105_000);

    let fixed = AuctionRules { bid_increment_ppm: 0, minimum_increment: 5_000, ..rules(2) };
    assert_eq!(fixed.next_minimum_bid(None, Some(current), true, false).unwrap().value(), 105_000);

    let override_start = amount(2_000);
    assert_eq!(
        v2.next_minimum_bid(Some(override_start), None, true, false).unwrap(),
        override_start
    );
}

#[test]
fn invalid_rules_and_bid_amounts_fail_closed() {
    assert!(matches!(
        AuctionRules { bid_increment_ppm: 0, minimum_increment: 0, ..rules(2) }.validate(),
        Err(DomainError::InvalidRules(_))
    ));
    assert!(matches!(
        AuctionRules { bid_increment_ppm: 0, minimum_increment: 500, ..rules(2) }.validate(),
        Err(DomainError::InvalidRules(_))
    ));
    assert!(matches!(
        AuctionRules { bidding_version: 3, ..rules(2) }.validate(),
        Err(DomainError::InvalidRules(_))
    ));

    assert!(matches!(BidAmount::new(1_500), Err(DomainError::InvalidAmount { .. })));
    assert_eq!(BidAmount::from_persisted_positive(1_500).unwrap().value(), 1_500);
    let ceiling = BidAmount::from_persisted_positive(MAX_BID_AMOUNT + 1_500).unwrap();
    assert_eq!(ceiling.bid_ceiling().value(), MAX_BID_AMOUNT);
    assert!(amount(1_000).is_valid_bid_input());
    assert!(!BidAmount::from_persisted_positive(1_500).unwrap().is_valid_bid_input());
    assert!(amount(MAX_BID_AMOUNT).is_valid_bid_input());
    assert!(!BidAmount::new(MAX_BID_AMOUNT + 1_000).unwrap().is_valid_bid_input());
}

#[test]
fn fast_proxy_strictly_outbids_an_ordinary_bid() {
    let lot_id = LotId::new();
    let proxy_user = UserId::new();
    let outsider = UserId::new();
    let proxy = active_proxy(lot_id, proxy_user, 150_000, ProxyMode::Fast);
    let state = lot_state(lot_id, Some(amount(100_000)), Some(proxy_user), at(12, 0, 0));

    let plan = CompetitionPlan::plan(
        rules(2),
        state,
        Some(proxy),
        ordinary_bid(outsider, amount(120_000), at(10, 0, 0), 2),
    )
    .expect("快速代理必须能反超普通出价");

    assert_eq!(plan.public_bids.len(), 2);
    assert_eq!(plan.public_bids[0].amount.value(), 120_000);
    assert_eq!(plan.public_bids[0].bidder_id, outsider);
    assert_eq!(plan.public_bids[0].mechanism, BidMechanism::Ordinary);
    assert_eq!(plan.public_bids[1].amount.value(), 126_000);
    assert_eq!(plan.public_bids[1].bidder_id, proxy_user);
    assert_eq!(plan.public_bids[1].mechanism, BidMechanism::Proxy);
    assert_eq!(plan.final_lot.current_price.unwrap().value(), 126_000);
    assert_eq!(plan.final_lot.leader_id, Some(proxy_user));
    assert_eq!(plan.active_proxy_id, Some(proxy.id));
    assert!(plan.proxy_transitions.is_empty());
    assert!(!plan.soft_close_applied);
}

#[test]
fn proxy_at_its_ceiling_is_exhausted_instead_of_bidding_again() {
    let lot_id = LotId::new();
    let proxy_user = UserId::new();
    let outsider = UserId::new();
    let proxy = active_proxy(lot_id, proxy_user, 150_000, ProxyMode::Fast);
    let state = lot_state(lot_id, Some(amount(100_000)), Some(proxy_user), at(12, 0, 0));

    let plan = CompetitionPlan::plan(
        rules(2),
        state,
        Some(proxy),
        ordinary_bid(outsider, amount(150_000), at(10, 0, 0), 1),
    )
    .expect("普通出价可以恰好达到代理上限");

    assert_eq!(plan.public_bids.len(), 1);
    assert_eq!(plan.public_bids[0].amount.value(), 150_000);
    assert_eq!(plan.final_lot.leader_id, Some(outsider));
    assert_eq!(plan.active_proxy_id, None);
    assert_eq!(plan.proxy_transitions.len(), 1);
    assert_eq!(plan.proxy_transitions[0].kind, ProxyTransitionKind::Exhausted);
    assert_eq!(plan.proxy_transitions[0].reason, Some(ProxyTerminalReason::StrictlyDefeated));
    assert_eq!(plan.proxy_states[0].status, ProxyStatus::Exhausted);
}

#[test]
fn legacy_v1_keeps_the_same_price_proxy_tie() {
    let lot_id = LotId::new();
    let proxy_user = UserId::new();
    let outsider = UserId::new();
    let proxy = active_proxy(lot_id, proxy_user, 120_000, ProxyMode::Fast);
    let state = lot_state(lot_id, Some(amount(100_000)), Some(proxy_user), at(12, 0, 0));

    let plan = CompetitionPlan::plan(
        rules(1),
        state,
        Some(proxy),
        ordinary_bid(outsider, amount(120_000), at(10, 0, 0), 2),
    )
    .expect("v1 同价代理事实必须保留");

    assert_eq!(plan.public_bids.len(), 2);
    assert_eq!(plan.public_bids[0].amount.value(), 120_000);
    assert_eq!(plan.public_bids[1].amount.value(), 120_000);
    assert_eq!(plan.public_bids[1].mechanism, BidMechanism::Proxy);
    assert_eq!(plan.final_lot.leader_id, Some(proxy_user));
}

#[test]
fn bids_reject_wrong_actor_low_amounts_same_price_and_closed_lots() {
    let lot_id = LotId::new();
    let leader = UserId::new();
    let other = UserId::new();
    let state = lot_state(lot_id, Some(amount(100_000)), Some(leader), at(12, 0, 0));

    assert!(matches!(
        CompetitionPlan::plan(
            rules(2),
            state,
            None,
            ordinary_bid(leader, amount(110_000), at(10, 0, 0), 1),
        ),
        Err(DomainError::AlreadyLeading)
    ));

    let mut empty = lot_state(lot_id, None, None, at(12, 0, 0));
    empty.start_price = amount(10_000);
    assert_eq!(
        CompetitionPlan::plan(
            rules(2),
            empty,
            None,
            ordinary_bid(other, amount(9_000), at(10, 0, 0), 1),
        ),
        Err(DomainError::BidTooLow { minimum: amount(10_000) })
    );

    // v2 普通出价与当前价相同不构成严格反超。
    assert!(matches!(
        CompetitionPlan::plan(
            rules(2),
            state,
            None,
            ordinary_bid(other, amount(100_000), at(10, 0, 0), 1),
        ),
        Err(DomainError::BidTooLow { .. })
    ));

    assert!(matches!(
        CompetitionPlan::plan(
            rules(2),
            state,
            None,
            ordinary_bid(other, amount(105_000), at(12, 0, 1), 1),
        ),
        Err(DomainError::AuctionClosed)
    ));
}

#[test]
fn soft_close_extends_only_inside_the_frozen_window() {
    let lot_id = LotId::new();
    let leader = UserId::new();
    let bidder = UserId::new();
    let state = lot_state(lot_id, Some(amount(100_000)), Some(leader), at(12, 0, 0));

    let inside = CompetitionPlan::plan(
        rules(2),
        state,
        None,
        ordinary_bid(bidder, amount(105_000), at(11, 50, 0), 1),
    )
    .expect("窗口内的有效出价必须延长");
    assert!(inside.soft_close_applied);
    assert_eq!(inside.final_lot.effective_end_at, at(12, 5, 0));
    assert_eq!(inside.public_bids[0].effective_end_at, at(12, 5, 0));

    let outside = CompetitionPlan::plan(
        rules(2),
        state,
        None,
        ordinary_bid(bidder, amount(105_000), at(11, 40, 0), 1),
    )
    .expect("窗口外出价不得改变截止");
    assert!(!outside.soft_close_applied);
    assert_eq!(outside.final_lot.effective_end_at, at(12, 0, 0));
}
