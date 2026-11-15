//! 代理竞价规划器（创建、降额、慢速到期、最终阶段、冻结与关闭）的纯逻辑回归。
//!
//! 期望值按 docs/business-rules.md 的“竞价”“代理竞价”两节推出：默认 5% 增量按千位向上
//! 取整（100,000→105,000、120,000→126,000、145,000→153,000、150,000→158,000），v1 自动
//! 代理反击与代理竞争固定 +1,000；软截止窗口与延长均为 15 分钟，慢速代理等待 10 分钟。
//! 测试只构造锁内快照、活动授权与触发器，不访问数据库或网络。

use auction_domain::{
    AuctionRules, AuctionStatus, BidAmount, BidId, BidMechanism, CommandId, CompetitionPlan,
    CompetitionTrigger, DEFAULT_START_PRICE, DomainError, LotBidState, LotId, ProxyAuthorization,
    ProxyAuthorizationId, ProxyMode, ProxyStatus, ProxyTerminalReason, ProxyTransitionKind, UserId,
};
use chrono::{DateTime, Utc};

fn must<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
    result.unwrap_or_else(|error| panic!("测试夹具必须合法: {error:?}"))
}

fn at(hour: u32, minute: u32) -> DateTime<Utc> {
    chrono::TimeZone::with_ymd_and_hms(&Utc, 2026, 1, 1, hour, minute, 0)
        .single()
        .unwrap_or_else(|| panic!("测试时间必须存在"))
}

fn amount(value: i64) -> BidAmount {
    must(BidAmount::new(value))
}

fn rules(version: u32) -> AuctionRules {
    AuctionRules { bidding_version: version, ..AuctionRules::default() }
}

/// lot 有效截止固定在 12:00；已有一份序号为 1 的授权历史。
fn lot_state(lot_id: LotId, current: Option<i64>, leader: Option<UserId>) -> LotBidState {
    LotBidState {
        lot_id,
        start_price: amount(DEFAULT_START_PRICE),
        current_price: current.map(amount),
        leader_id: leader,
        leader_proxy_last: false,
        material: false,
        effective_end_at: at(12, 0),
        state_version: 1,
        last_proxy_sequence: 1,
        slow_generation: 0,
        slow_authorization_id: None,
        slow_due_at: None,
    }
}

/// 慢速代理被 `leader` 超越后等待反击：第 1 代调度在 `due_at` 到期。
fn waiting_state(
    proxy: &ProxyAuthorization,
    current: i64,
    leader: UserId,
    due_at: DateTime<Utc>,
) -> LotBidState {
    LotBidState {
        slow_generation: 1,
        slow_authorization_id: Some(proxy.id),
        slow_due_at: Some(due_at),
        ..lot_state(proxy.lot_id, Some(current), Some(leader))
    }
}

fn proxy(lot_id: LotId, user_id: UserId, maximum: i64, mode: ProxyMode) -> ProxyAuthorization {
    must(ProxyAuthorization::new(
        ProxyAuthorizationId::new(),
        lot_id,
        user_id,
        CommandId::new(),
        1,
        1,
        amount(maximum),
        mode,
    ))
}

struct Create {
    actor: UserId,
    maximum: i64,
    mode: ProxyMode,
    auction_status: AuctionStatus,
    usage_number: u64,
    previous: Option<(i64, ProxyStatus)>,
}

impl Create {
    fn new(actor: UserId, maximum: i64) -> Self {
        Self {
            actor,
            maximum,
            mode: ProxyMode::Fast,
            auction_status: AuctionStatus::Live,
            usage_number: 1,
            previous: None,
        }
    }

    fn trigger(&self, authorization_id: ProxyAuthorizationId) -> CompetitionTrigger {
        CompetitionTrigger::CreateProxy {
            command_id: CommandId::new(),
            actor_id: self.actor,
            authorization_id,
            maximum_amount: amount(self.maximum),
            mode: self.mode,
            auction_status: self.auction_status,
            authorization_sequence: 2,
            usage_number: self.usage_number,
            previous_maximum: self.previous.map(|(maximum, _)| amount(maximum)),
            previous_status: self.previous.map(|(_, status)| status),
            bid_ids: vec![BidId::new()],
            now: at(10, 0),
        }
    }
}

fn lower(proxy: &ProxyAuthorization, new_maximum: i64) -> CompetitionTrigger {
    CompetitionTrigger::LowerProxy {
        command_id: CommandId::new(),
        actor_id: proxy.user_id,
        authorization_id: proxy.id,
        new_maximum: amount(new_maximum),
        now: at(10, 0),
    }
}

fn slow_due(
    authorization_id: ProxyAuthorizationId,
    generation: u64,
    due_at: DateTime<Utc>,
    now: DateTime<Utc>,
) -> CompetitionTrigger {
    CompetitionTrigger::SlowDue {
        command_id: CommandId::new(),
        authorization_id,
        generation,
        due_at,
        bid_ids: vec![BidId::new()],
        now,
    }
}

fn final_proxy(authorization_id: ProxyAuthorizationId, bid_count: usize) -> CompetitionTrigger {
    CompetitionTrigger::FinalProxy {
        command_id: CommandId::new(),
        authorization_id,
        bid_ids: (0..bid_count).map(|_| BidId::new()).collect(),
        now: at(11, 0),
    }
}

fn bids(plan: &CompetitionPlan) -> Vec<(UserId, i64, BidMechanism)> {
    plan.public_bids.iter().map(|bid| (bid.bidder_id, bid.amount.value(), bid.mechanism)).collect()
}

fn transitions(
    plan: &CompetitionPlan,
) -> Vec<(ProxyAuthorizationId, ProxyTransitionKind, Option<ProxyTerminalReason>)> {
    plan.proxy_transitions
        .iter()
        .map(|transition| (transition.authorization_id, transition.kind, transition.reason))
        .collect()
}

fn assert_schedule_cleared(plan: &CompetitionPlan, before: LotBidState) {
    assert_eq!(plan.final_lot.slow_authorization_id, None);
    assert_eq!(plan.final_lot.slow_due_at, None);
    // 世代前进才能让 Worker 手里的旧候选失效。
    assert!(plan.final_lot.slow_generation > before.slow_generation);
}

#[test]
fn creating_without_competition_bids_the_current_minimum() {
    for version in [1, 2] {
        let lot_id = LotId::new();
        let (leader, actor) = (UserId::new(), UserId::new());
        let authorization_id = ProxyAuthorizationId::new();
        let state = lot_state(lot_id, Some(100_000), Some(leader));

        let plan = must(CompetitionPlan::plan(
            rules(version),
            state,
            None,
            Create::new(actor, 150_000).trigger(authorization_id),
        ));

        // 最低可出价 100,000 + 5% = 105,000；新代理以该价领先，上限不是出价事实。
        assert_eq!(bids(&plan), vec![(actor, 105_000, BidMechanism::Proxy)], "v{version}");
        assert_eq!(
            transitions(&plan),
            vec![(authorization_id, ProxyTransitionKind::Created, None)]
        );
        assert_eq!(plan.proxy_states.len(), 1);
        assert_eq!(plan.proxy_states[0].status, ProxyStatus::Active);
        assert_eq!(plan.proxy_states[0].maximum_amount, amount(150_000));
        assert_eq!(plan.active_proxy_id, Some(authorization_id));
        assert_eq!(plan.final_lot.leader_id, Some(actor));
        assert!(plan.final_lot.leader_proxy_last);
        assert_eq!(plan.final_lot.last_proxy_sequence, 2);
        assert!(!plan.soft_close_applied);
    }

    // 无人出价时最低可出价就是起拍价。
    let actor = UserId::new();
    let plan = must(CompetitionPlan::plan(
        rules(2),
        lot_state(LotId::new(), None, None),
        None,
        Create::new(actor, 50_000).trigger(ProxyAuthorizationId::new()),
    ));
    assert_eq!(bids(&plan), vec![(actor, DEFAULT_START_PRICE, BidMechanism::Proxy)]);
}

#[test]
fn creating_rejects_low_ceilings_leaders_disabled_proxies_and_final_slow() {
    let lot_id = LotId::new();
    let (leader, actor) = (UserId::new(), UserId::new());
    let state = lot_state(lot_id, Some(100_000), Some(leader));
    let plan_create = |create: Create| {
        CompetitionPlan::plan(rules(2), state, None, create.trigger(ProxyAuthorizationId::new()))
    };

    // 上限必须达到创建时的最低可出价 105,000。
    assert_eq!(
        plan_create(Create::new(actor, 104_000)),
        Err(DomainError::BidTooLow { minimum: amount(105_000) })
    );
    assert_eq!(plan_create(Create::new(leader, 150_000)), Err(DomainError::AlreadyLeading));
    assert_eq!(
        CompetitionPlan::plan(
            AuctionRules { proxy_enabled: false, ..rules(2) },
            state,
            None,
            Create::new(actor, 150_000).trigger(ProxyAuthorizationId::new()),
        ),
        Err(DomainError::ProxyDisabled)
    );
    // Final 阶段不能新建 Slow 代理，Fast 代理仍可创建。
    let final_slow = Create {
        mode: ProxyMode::Slow,
        auction_status: AuctionStatus::Final,
        ..Create::new(actor, 150_000)
    };
    assert_eq!(plan_create(final_slow), Err(DomainError::ProxySlowFinal));
    let final_fast = Create { auction_status: AuctionStatus::Final, ..Create::new(actor, 150_000) };
    assert!(plan_create(final_fast).is_ok());

    // 等待反击的慢速代理持有人不能在同一 lot 再建一份代理。
    let holder = UserId::new();
    let waiting = proxy(lot_id, holder, 150_000, ProxyMode::Slow);
    assert_eq!(
        CompetitionPlan::plan(
            rules(2),
            waiting_state(&waiting, 120_000, leader, at(10, 10)),
            Some(waiting),
            Create::new(holder, 160_000).trigger(ProxyAuthorizationId::new()),
        ),
        Err(DomainError::ActiveProxyExists)
    );
}

#[test]
fn competing_proxies_rank_by_ceiling_and_record_one_winner_bid() {
    // (新上限, v1 竞争价, v2 竞争价, 新代理是否胜出)
    // 竞争价 = 较低上限 + 本场增量（v1 固定 +1,000），再受胜者上限约束。
    let cases = [
        // 新上限更高：150,000 + 5% = 158,000；v1 为 151,000。
        (200_000, 151_000, 158_000, true),
        // 新上限更低：120,000 + 5% = 126,000；v1 为 121,000。
        (120_000, 121_000, 126_000, false),
        // 同上限由更早授权者优先，竞争价被胜者上限截到 150,000。
        (150_000, 150_000, 150_000, false),
    ];
    for version in [1, 2] {
        for (new_maximum, v1_price, v2_price, candidate_wins) in cases {
            let lot_id = LotId::new();
            let (incumbent, challenger) = (UserId::new(), UserId::new());
            let active = proxy(lot_id, incumbent, 150_000, ProxyMode::Fast);
            let state = lot_state(lot_id, Some(100_000), Some(incumbent));
            let created_id = ProxyAuthorizationId::new();

            let plan = must(CompetitionPlan::plan(
                rules(version),
                state,
                Some(active),
                Create::new(challenger, new_maximum).trigger(created_id),
            ));

            let price = if version == 1 { v1_price } else { v2_price };
            let (winner, winner_id, loser_id) = if candidate_wins {
                (challenger, created_id, active.id)
            } else {
                (incumbent, active.id, created_id)
            };
            let label = format!("v{version} 新上限 {new_maximum}");
            assert_eq!(bids(&plan), vec![(winner, price, BidMechanism::Proxy)], "{label}");
            assert_eq!(
                transitions(&plan),
                vec![
                    (created_id, ProxyTransitionKind::Created, None),
                    (
                        loser_id,
                        ProxyTransitionKind::Exhausted,
                        Some(ProxyTerminalReason::RankedBelow)
                    ),
                ],
                "{label}"
            );
            assert_eq!(plan.active_proxy_id, Some(winner_id), "{label}");
            let loser_state = plan.proxy_states.iter().find(|state| state.id == loser_id);
            assert_eq!(loser_state.map(|state| state.status), Some(ProxyStatus::Exhausted));
            assert_eq!(plan.final_lot.leader_id, Some(winner), "{label}");
            assert_eq!(plan.final_lot.current_price, Some(amount(price)), "{label}");
        }
    }
}

#[test]
fn reentry_requires_quota_consistent_history_and_a_higher_ceiling() {
    let lot_id = LotId::new();
    let (leader, actor) = (UserId::new(), UserId::new());
    let state = lot_state(lot_id, Some(120_000), Some(leader));
    let reentry = |usage_number, previous, maximum| Create {
        usage_number,
        previous,
        ..Create::new(actor, maximum)
    };
    let plan_with = |auction_rules: AuctionRules, create: Create| {
        CompetitionPlan::plan(
            auction_rules,
            state,
            None,
            create.trigger(ProxyAuthorizationId::new()),
        )
    };

    // 默认每人每 lot 最多 3 次；0 表示不限。
    let fourth = || reentry(4, Some((125_000, ProxyStatus::Exhausted)), 130_000);
    assert_eq!(plan_with(rules(2), fourth()), Err(DomainError::ProxyOutbidLimit));
    assert!(plan_with(AuctionRules { proxy_max_outbid_uses: 0, ..rules(2) }, fourth()).is_ok());

    // 使用次数必须与上一份授权是否存在一致。
    assert!(matches!(
        plan_with(rules(2), reentry(1, Some((125_000, ProxyStatus::Exhausted)), 130_000)),
        Err(DomainError::InvalidBidState(_))
    ));
    assert!(matches!(
        plan_with(rules(2), reentry(2, None, 130_000)),
        Err(DomainError::InvalidBidState(_))
    ));

    // 上一份仍生效时不能重设，竞争已关闭时不能再建。
    for status in [ProxyStatus::Active, ProxyStatus::FinalFast] {
        assert_eq!(
            plan_with(rules(2), reentry(2, Some((125_000, status)), 130_000)),
            Err(DomainError::ActiveProxyExists)
        );
    }
    assert_eq!(
        plan_with(rules(2), reentry(2, Some((125_000, ProxyStatus::Closed)), 130_000)),
        Err(DomainError::AuctionClosed)
    );

    // 被耗尽或冻结后可以重设，但新上限必须高于上一份。
    for previous_maximum in [130_000, 140_000] {
        assert_eq!(
            plan_with(
                rules(2),
                reentry(2, Some((previous_maximum, ProxyStatus::Exhausted)), 130_000)
            ),
            Err(DomainError::ProxyReentryMaximumNotHigher)
        );
    }
    for status in [ProxyStatus::Exhausted, ProxyStatus::Frozen] {
        let plan = must(plan_with(rules(2), reentry(2, Some((125_000, status)), 130_000)));
        // 120,000 + 5% = 126,000。
        assert_eq!(bids(&plan), vec![(actor, 126_000, BidMechanism::Proxy)]);
        assert_eq!(plan.proxy_states[0].usage_number, 2);
    }
}

#[test]
fn lowering_respects_the_current_or_waiting_binding_amount() {
    let lot_id = LotId::new();
    let holder = UserId::new();
    let leading = proxy(lot_id, holder, 150_000, ProxyMode::Fast);
    let state = lot_state(lot_id, Some(120_000), Some(holder));
    let lower_to = |new_maximum| {
        CompetitionPlan::plan(rules(2), state, Some(leading), lower(&leading, new_maximum))
    };

    // 领先时不低于当前价，恰好等于当前价也可以。
    for new_maximum in [130_000, 120_000] {
        let plan = must(lower_to(new_maximum));
        assert!(plan.public_bids.is_empty());
        assert_eq!(transitions(&plan), vec![(leading.id, ProxyTransitionKind::Lowered, None)]);
        assert_eq!(plan.proxy_transitions[0].old_maximum, Some(amount(150_000)));
        assert_eq!(plan.proxy_states[0].maximum_amount, amount(new_maximum));
        assert_eq!(plan.proxy_states[0].status, ProxyStatus::Active);
        assert_eq!(plan.active_proxy_id, Some(leading.id));
        assert_eq!(plan.final_lot, state);
    }
    assert_eq!(lower_to(119_000), Err(DomainError::ProxyMaximumBelowBinding));
    // 只能降低上限。
    for new_maximum in [150_000, 160_000] {
        assert_eq!(lower_to(new_maximum), Err(DomainError::ProxyMaximumNotLower));
    }

    // 授权、持有人或活动授权不符时一律拒绝。
    let stranger = UserId::new();
    let foreign = CompetitionTrigger::LowerProxy {
        command_id: CommandId::new(),
        actor_id: stranger,
        authorization_id: leading.id,
        new_maximum: amount(130_000),
        now: at(10, 0),
    };
    let unknown = CompetitionTrigger::LowerProxy {
        command_id: CommandId::new(),
        actor_id: holder,
        authorization_id: ProxyAuthorizationId::new(),
        new_maximum: amount(130_000),
        now: at(10, 0),
    };
    for trigger in [foreign, unknown] {
        assert_eq!(
            CompetitionPlan::plan(rules(2), state, Some(leading), trigger),
            Err(DomainError::ProxyAuthorizationNotActive)
        );
    }
    assert_eq!(
        CompetitionPlan::plan(
            rules(2),
            lot_state(lot_id, Some(120_000), Some(holder)),
            None,
            lower(&leading, 130_000)
        ),
        Err(DomainError::ProxyAuthorizationNotActive)
    );

    // 等待反击时不低于冻结策略要求的下一口：v2 为 126,000，v1 为 121,000。
    for (version, binding) in [(1, 121_000), (2, 126_000)] {
        let waiting = proxy(lot_id, holder, 150_000, ProxyMode::Slow);
        let state = waiting_state(&waiting, 120_000, UserId::new(), at(10, 10));
        let lower_to = |new_maximum| {
            CompetitionPlan::plan(
                rules(version),
                state,
                Some(waiting),
                lower(&waiting, new_maximum),
            )
        };
        let plan = must(lower_to(binding));
        assert_eq!(plan.proxy_states[0].maximum_amount, amount(binding));
        // 降额不改变等待调度。
        assert_eq!(plan.final_lot.slow_authorization_id, Some(waiting.id));
        assert_eq!(
            lower_to(binding - 1_000),
            Err(DomainError::ProxyMaximumBelowBinding),
            "v{version}"
        );
    }
}

#[test]
fn slow_proxy_counters_at_due_time_within_its_ceiling() {
    let due_at = at(10, 10);
    // 到期重算最低可出价：v2 120,000 + 5% = 126,000，v1 固定 +1,000。
    for (version, response) in [(1, 121_000), (2, 126_000)] {
        let lot_id = LotId::new();
        let (holder, rival) = (UserId::new(), UserId::new());
        let waiting = proxy(lot_id, holder, 150_000, ProxyMode::Slow);
        let state = waiting_state(&waiting, 120_000, rival, due_at);

        let plan = must(CompetitionPlan::plan(
            rules(version),
            state,
            Some(waiting),
            slow_due(waiting.id, 1, due_at, due_at),
        ));

        assert_eq!(bids(&plan), vec![(holder, response, BidMechanism::Proxy)], "v{version}");
        assert!(plan.proxy_transitions.is_empty());
        assert_eq!(plan.active_proxy_id, Some(waiting.id));
        assert_eq!(plan.final_lot.leader_id, Some(holder));
        assert_schedule_cleared(&plan, state);
        assert!(!plan.soft_close_applied);
    }

    // 最低加价 145,000 + 5% = 153,000 超过上限时直接以上限 150,000 出价。
    let lot_id = LotId::new();
    let (holder, rival) = (UserId::new(), UserId::new());
    let waiting = proxy(lot_id, holder, 150_000, ProxyMode::Slow);
    let plan = must(CompetitionPlan::plan(
        rules(2),
        waiting_state(&waiting, 145_000, rival, due_at),
        Some(waiting),
        slow_due(waiting.id, 1, due_at, due_at),
    ));
    assert_eq!(bids(&plan), vec![(holder, 150_000, BidMechanism::Proxy)]);

    // 当前价已等于上限时无法严格反超：只耗尽代理，不追加出价。
    let state = waiting_state(&waiting, 150_000, rival, due_at);
    let plan = must(CompetitionPlan::plan(
        rules(2),
        state,
        Some(waiting),
        slow_due(waiting.id, 1, due_at, due_at),
    ));
    assert!(plan.public_bids.is_empty());
    assert_eq!(
        transitions(&plan),
        vec![(
            waiting.id,
            ProxyTransitionKind::Exhausted,
            Some(ProxyTerminalReason::StrictlyDefeated)
        )]
    );
    assert_eq!(plan.active_proxy_id, None);
    assert_eq!(plan.final_lot.leader_id, Some(rival));
    assert_schedule_cleared(&plan, state);
}

#[test]
fn slow_proxy_rejects_stale_or_early_due_triggers() {
    let lot_id = LotId::new();
    let holder = UserId::new();
    let due_at = at(10, 10);
    let waiting = proxy(lot_id, holder, 150_000, ProxyMode::Slow);
    let state = waiting_state(&waiting, 120_000, UserId::new(), due_at);
    for trigger in [
        // 世代、到期时间或授权与锁内调度不一致，以及尚未到期。
        slow_due(waiting.id, 2, due_at, due_at),
        slow_due(waiting.id, 1, at(10, 11), at(10, 11)),
        slow_due(ProxyAuthorizationId::new(), 1, due_at, due_at),
        slow_due(waiting.id, 1, due_at, at(10, 9)),
    ] {
        assert!(matches!(
            CompetitionPlan::plan(rules(2), state, Some(waiting), trigger),
            Err(DomainError::InvalidBidState(_))
        ));
    }
}

#[test]
fn slow_proxy_bid_inside_the_soft_close_window_extends_the_lot() {
    let lot_id = LotId::new();
    let holder = UserId::new();
    let due_at = at(11, 50);
    let waiting = proxy(lot_id, holder, 150_000, ProxyMode::Slow);
    let plan = must(CompetitionPlan::plan(
        rules(2),
        waiting_state(&waiting, 120_000, UserId::new(), due_at),
        Some(waiting),
        slow_due(waiting.id, 1, due_at, at(11, 55)),
    ));
    // 截止前 15 分钟内接受的出价把截止延到“接受时刻 + 15 分钟”。
    assert!(plan.soft_close_applied);
    assert_eq!(plan.final_lot.effective_end_at, at(12, 10));
    assert_eq!(plan.public_bids[0].effective_end_at, at(12, 10));
}

#[test]
fn entering_final_converts_waiting_slow_proxies_to_final_fast() {
    // 等待中的慢速代理立即反击并转为 FinalFast。
    for (version, response) in [(1, 121_000), (2, 126_000)] {
        let lot_id = LotId::new();
        let (holder, rival) = (UserId::new(), UserId::new());
        let waiting = proxy(lot_id, holder, 150_000, ProxyMode::Slow);
        let state = waiting_state(&waiting, 120_000, rival, at(11, 5));

        let plan = must(CompetitionPlan::plan(
            rules(version),
            state,
            Some(waiting),
            final_proxy(waiting.id, 1),
        ));

        assert_eq!(bids(&plan), vec![(holder, response, BidMechanism::Proxy)], "v{version}");
        assert_eq!(transitions(&plan), vec![(waiting.id, ProxyTransitionKind::FinalFast, None)]);
        assert_eq!(plan.proxy_states[0].status, ProxyStatus::FinalFast);
        assert_eq!(plan.proxy_states[0].mode, ProxyMode::Fast);
        assert_eq!(plan.active_proxy_id, Some(waiting.id));
        assert_schedule_cleared(&plan, state);
    }

    // 未在等待的慢速代理只转换模式，不产生出价事实。
    let lot_id = LotId::new();
    let holder = UserId::new();
    let leading = proxy(lot_id, holder, 150_000, ProxyMode::Slow);
    let state = lot_state(lot_id, Some(120_000), Some(holder));
    let plan =
        must(CompetitionPlan::plan(rules(2), state, Some(leading), final_proxy(leading.id, 0)));
    assert!(plan.public_bids.is_empty());
    assert_eq!(transitions(&plan), vec![(leading.id, ProxyTransitionKind::FinalFast, None)]);
    assert_eq!(plan.final_lot.current_price, Some(amount(120_000)));
    assert!(matches!(
        CompetitionPlan::plan(rules(2), state, Some(leading), final_proxy(leading.id, 1)),
        Err(DomainError::InvalidBidState(_))
    ));

    // 无法严格反超的等待代理只耗尽，不再转换为 FinalFast。
    let rival = UserId::new();
    let waiting = proxy(lot_id, holder, 150_000, ProxyMode::Slow);
    let plan = must(CompetitionPlan::plan(
        rules(2),
        waiting_state(&waiting, 150_000, rival, at(11, 5)),
        Some(waiting),
        final_proxy(waiting.id, 1),
    ));
    assert!(plan.public_bids.is_empty());
    assert_eq!(
        transitions(&plan),
        vec![(
            waiting.id,
            ProxyTransitionKind::Exhausted,
            Some(ProxyTerminalReason::StrictlyDefeated)
        )]
    );
    assert_eq!(plan.active_proxy_id, None);

    // 原有 Fast 代理保持 Fast，不能经 FinalProxy 转换。
    let fast = proxy(lot_id, holder, 150_000, ProxyMode::Fast);
    assert!(matches!(
        CompetitionPlan::plan(
            rules(2),
            lot_state(lot_id, Some(120_000), Some(holder)),
            Some(fast),
            final_proxy(fast.id, 0),
        ),
        Err(DomainError::InvalidBidState(_))
    ));
}

#[test]
fn freezing_and_closing_end_the_proxy_and_clear_its_schedule() {
    let lot_id = LotId::new();
    let (holder, rival) = (UserId::new(), UserId::new());
    let waiting = proxy(lot_id, holder, 150_000, ProxyMode::Slow);
    let state = waiting_state(&waiting, 120_000, rival, at(10, 10));
    let terminal = |freeze: bool, authorization_id| {
        let (command_id, now) = (CommandId::new(), at(10, 5));
        if freeze {
            CompetitionTrigger::FreezeProxy { command_id, authorization_id, now }
        } else {
            CompetitionTrigger::CloseProxy { command_id, authorization_id, now }
        }
    };

    for (freeze, kind, status, reason) in [
        (
            true,
            ProxyTransitionKind::Frozen,
            ProxyStatus::Frozen,
            ProxyTerminalReason::EligibilityLost,
        ),
        (
            false,
            ProxyTransitionKind::Closed,
            ProxyStatus::Closed,
            ProxyTerminalReason::CompetitionClosed,
        ),
    ] {
        let plan = must(CompetitionPlan::plan(
            rules(2),
            state,
            Some(waiting),
            terminal(freeze, waiting.id),
        ));
        assert!(plan.public_bids.is_empty());
        assert_eq!(transitions(&plan), vec![(waiting.id, kind, Some(reason))]);
        assert_eq!(plan.proxy_states[0].status, status);
        assert_eq!(plan.active_proxy_id, None);
        assert_eq!(plan.final_lot.leader_id, Some(rival));
        assert_schedule_cleared(&plan, state);

        // 授权不符或没有活动授权时不能终止任何代理。
        assert!(matches!(
            CompetitionPlan::plan(
                rules(2),
                state,
                Some(waiting),
                terminal(freeze, ProxyAuthorizationId::new())
            ),
            Err(DomainError::InvalidProxyTransition { .. })
        ));
        assert!(matches!(
            CompetitionPlan::plan(
                rules(2),
                lot_state(lot_id, Some(120_000), Some(rival)),
                None,
                terminal(freeze, waiting.id)
            ),
            Err(DomainError::InvalidProxyTransition { .. })
        ));
    }
}
