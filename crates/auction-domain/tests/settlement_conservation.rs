//! 结算守恒、持有曲线插值与补偿池冲正的纯逻辑回归。
//!
//! 只使用公开领域 API，不接触数据库或应用服务；金额全部按下述冻结规则计算：
//! PPM 向下取整、比例分配每件向下取整且尾差留池、v2 卖方始终全额收款。

use auction_domain::{
    AdjustmentAnchor, COMPENSATION_REVERSAL_RULE_VERSION, CompensationReversal, DomainError,
    EquipmentId, LotId, SETTLEMENT_PPM, SettlementAmount, SettlementFact, SettlementMode,
    SettlementPlan, SettlementPolicy, SettlementWorld, UserId, floor_ppm, proportional_floor,
};
use chrono::{DateTime, TimeZone, Utc};

fn must<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
    result.unwrap_or_else(|error| panic!("测试夹具必须合法: {error:?}"))
}

fn required<T>(value: Option<T>) -> T {
    value.unwrap_or_else(|| panic!("测试夹具必须存在"))
}

fn at(year: i32, month: u32, day: u32, hour: u32, minute: u32, second: u32) -> DateTime<Utc> {
    required(Utc.with_ymd_and_hms(year, month, day, hour, minute, second).single())
}

fn window_open() -> DateTime<Utc> {
    at(2026, 1, 1, 0, 0, 0)
}

fn window_end() -> DateTime<Utc> {
    at(2026, 1, 11, 0, 0, 0)
}

/// 十天的计划窗口，用于把持有时长 ppm 精确换算成累计领先纳秒。
const PLANNED_NANOS: i128 = 10 * 24 * 60 * 60 * 1_000_000_000;

fn winning_fact(price: i64, held_ppm: i64) -> SettlementFact {
    SettlementFact {
        lot_id: LotId::new(),
        world: SettlementWorld::Main,
        end_price: Some(must(SettlementAmount::new(price))),
        leader_id: Some(UserId::new()),
        leader_since: Some(window_open()),
        leader_held_nanos: Some(must(i64::try_from(
            PLANNED_NANOS * i128::from(held_ppm) / 1_000_000,
        ))),
        effective_end_at: window_end(),
        seller_identity: "seller".to_owned(),
        equipment_id: EquipmentId::new(),
        submission_id: None,
    }
}

fn unsold_fact() -> SettlementFact {
    SettlementFact {
        lot_id: LotId::new(),
        world: SettlementWorld::Main,
        end_price: None,
        leader_id: None,
        leader_since: None,
        leader_held_nanos: None,
        effective_end_at: window_end(),
        seller_identity: "seller".to_owned(),
        equipment_id: EquipmentId::new(),
        submission_id: None,
    }
}

fn v1_policy() -> SettlementPolicy {
    SettlementPolicy {
        version: 1,
        mode: SettlementMode::HoldCurveV1,
        ..SettlementPolicy::default()
    }
}

#[test]
fn settlement_plan_conserves_each_line_and_the_compensation_pool() {
    let policy = SettlementPolicy::default();
    // 0 ppm 持有 → +80000 ppm 附加费；250000 → 0；300000 → -4000 折扣。
    let facts =
        [winning_fact(100_000, 0), winning_fact(200_000, 250_000), winning_fact(300_000, 300_000)];

    let plan = SettlementPlan::calculate(
        &policy,
        window_open(),
        window_end(),
        SettlementAmount::zero(),
        &facts,
    )
    .expect("合法的 v2 结算事实必须能生成计划");

    assert_eq!(plan.tax_total.value(), 8_000);
    assert_eq!(plan.discount_demand.value(), 1_200);
    // 附加费形成 8000 的池，名义折扣 1200 全额可分配，尾差留池。
    assert_eq!(plan.compensation_total.value(), 1_200);
    assert_eq!(plan.closing_pool.value(), 6_800);
    assert_eq!(plan.lines.len(), 3);

    for line in &plan.lines {
        assert_eq!(
            line.buyer_amount.value() + line.compensation_amount.value(),
            line.seller_amount.value() + line.tax_amount.value(),
            "每一行必须满足买方支付加补偿等于卖方收入加税费"
        );
        // v2 卖方始终按成交价全额收款，优惠由池承担。
        assert_eq!(line.seller_amount, line.end_price);
    }
    assert_eq!(plan.lines[0].adjustment_ppm, 80_000);
    assert_eq!(plan.lines[0].buyer_amount.value(), 108_000);
    assert_eq!(plan.lines[1].adjustment_ppm, 0);
    assert_eq!(plan.lines[2].adjustment_ppm, -4_000);
    assert_eq!(plan.lines[2].listed_price.value(), 298_800);
    assert_eq!(plan.lines[2].buyer_amount.value(), 298_800);

    plan.validate(&policy).expect("重新校验必须接受自己生成的计划");
}

#[test]
fn unsold_lot_keeps_zero_amounts_and_never_touches_the_pool() {
    let policy = SettlementPolicy::default();
    let plan = SettlementPlan::calculate(
        &policy,
        window_open(),
        window_end(),
        SettlementAmount::new(500).unwrap(),
        &[unsold_fact()],
    )
    .expect("流拍事实必须能结算");

    assert_eq!(plan.lines.len(), 1);
    let line = &plan.lines[0];
    assert_eq!(line.end_price.value(), 0);
    assert_eq!(line.buyer_amount.value(), 0);
    assert_eq!(line.seller_amount.value(), 0);
    assert_eq!(plan.tax_total.value(), 0);
    assert_eq!(plan.compensation_total.value(), 0);
    assert_eq!(plan.closing_pool.value(), 500);
}

#[test]
fn v1_keeps_the_seller_concession_and_extra_pool_credit() {
    let policy = v1_policy();
    let fact = winning_fact(300_000, 300_000);
    let plan = SettlementPlan::calculate(
        &policy,
        window_open(),
        window_end(),
        SettlementAmount::new(100).unwrap(),
        &[fact],
    )
    .expect("冻结的 v1 快照仍须可结算");

    let line = &plan.lines[0];
    assert_eq!(line.adjustment_ppm, -4_000);
    assert_eq!(line.listed_price.value(), 298_800);
    // v1 卖方让利到挂牌价，池再追加一份被名义折扣封顶的抵扣。
    assert_eq!(line.seller_amount.value(), 298_800);
    assert_eq!(line.compensation_amount.value(), 100);
    assert_eq!(line.buyer_amount.value(), 298_700);
    assert_eq!(plan.closing_pool.value(), 0);
    plan.validate(&policy).expect("v1 计划必须保持冻结模式矩阵");
}

#[test]
fn hold_curve_interpolates_with_half_away_from_zero_rounding() {
    // 0→0、1000 ppm→+1 ppm，其余曲线段保持单调；最后锚点必须是 1e6。
    let policy = SettlementPolicy {
        anchors: vec![
            AdjustmentAnchor { hold_ppm: 0, adjustment_ppm: 0 },
            AdjustmentAnchor { hold_ppm: 1_000, adjustment_ppm: 1 },
            AdjustmentAnchor { hold_ppm: SETTLEMENT_PPM, adjustment_ppm: -1 },
        ],
        ..SettlementPolicy::default()
    };
    let open = at(2026, 1, 1, 0, 0, 0);
    let end = at(2026, 1, 1, 0, 0, 1);
    let planned_nanos = 1_000_000_000;

    let adjusted = |held_nanos: i64| {
        let fact = SettlementFact {
            lot_id: LotId::new(),
            world: SettlementWorld::Main,
            end_price: Some(SettlementAmount::new(1_000_000).unwrap()),
            leader_id: Some(UserId::new()),
            leader_since: Some(open),
            leader_held_nanos: Some(held_nanos),
            effective_end_at: end,
            seller_identity: "seller".to_owned(),
            equipment_id: EquipmentId::new(),
            submission_id: None,
        };
        let plan = SettlementPlan::calculate(&policy, open, end, SettlementAmount::zero(), &[fact])
            .expect("单秒窗口内的合法事实必须能结算");
        plan.lines[0].adjustment_ppm
    };

    // 499 ppm 的插值余数不到一半，向下取整；500 与 750 都按远离零取整到 1。
    assert_eq!(adjusted(planned_nanos * 499 / 1_000_000), 0);
    assert_eq!(adjusted(planned_nanos * 500 / 1_000_000), 1);
    assert_eq!(adjusted(planned_nanos * 750 / 1_000_000), 1);
    // 命中右锚点时直接返回锚点值，不进行插值。
    assert_eq!(adjusted(planned_nanos), -1);
}

#[test]
fn proportional_floor_keeps_remainders_in_the_pool() {
    let available = SettlementAmount::new(100).unwrap();
    let demand = SettlementAmount::new(1).unwrap();
    let total = SettlementAmount::new(3).unwrap();
    assert_eq!(proportional_floor(available, demand, total).unwrap().value(), 33);

    // 尾差不能被向上补足，否则全场支出会超过可用池。
    let sum: i64 =
        (0..3).map(|_| proportional_floor(available, demand, total).unwrap().value()).sum();
    assert_eq!(sum, 99);

    assert!(matches!(
        proportional_floor(available, demand, SettlementAmount::zero()),
        Err(DomainError::InvalidSettlementFacts(_))
    ));
}

#[test]
fn floor_ppm_and_amount_helpers_fail_closed() {
    let amount = SettlementAmount::new(1_999).unwrap();
    assert_eq!(floor_ppm(amount, 500_000).unwrap().value(), 999);
    assert_eq!(floor_ppm(amount, SETTLEMENT_PPM).unwrap(), amount);
    assert!(matches!(
        floor_ppm(amount, SETTLEMENT_PPM + 1),
        Err(DomainError::InvalidSettlementFacts(_))
    ));
    assert!(matches!(SettlementAmount::new(-1), Err(DomainError::InvalidSettlementFacts(_))));
    assert!(matches!(
        SettlementAmount::new(1).unwrap().checked_sub(SettlementAmount::new(2).unwrap()),
        Err(DomainError::SettlementAmountOverflow)
    ));
}

#[test]
fn settlement_plan_rejects_invalid_windows_facts_and_policy_shapes() {
    let policy = SettlementPolicy::default();
    assert!(matches!(
        SettlementPlan::calculate(
            &policy,
            window_open(),
            window_end(),
            SettlementAmount::zero(),
            &[],
        ),
        Err(DomainError::InvalidSettlementFacts(_))
    ));
    assert!(matches!(
        SettlementPlan::calculate(
            &policy,
            window_end(),
            window_open(),
            SettlementAmount::zero(),
            &[winning_fact(1_000, 0)],
        ),
        Err(DomainError::InvalidSettlementFacts(_))
    ));

    let mut missing_held = winning_fact(1_000, 0);
    missing_held.leader_held_nanos = None;
    assert!(matches!(
        SettlementPlan::calculate(
            &policy,
            window_open(),
            window_end(),
            SettlementAmount::zero(),
            &[missing_held],
        ),
        Err(DomainError::InvalidSettlementFacts(_))
    ));

    let mut contradictory = unsold_fact();
    contradictory.leader_id = Some(UserId::new());
    contradictory.leader_since = Some(window_open());
    assert!(matches!(
        SettlementPlan::calculate(
            &policy,
            window_open(),
            window_end(),
            SettlementAmount::zero(),
            &[contradictory],
        ),
        Err(DomainError::InvalidSettlementFacts(_))
    ));

    // 锚点必须严格递增，且首尾固定为 0 / 1e6。
    let non_monotonic = SettlementPolicy {
        anchors: vec![
            AdjustmentAnchor { hold_ppm: 0, adjustment_ppm: 0 },
            AdjustmentAnchor { hold_ppm: 1_000, adjustment_ppm: 0 },
            AdjustmentAnchor { hold_ppm: 1_000, adjustment_ppm: 1 },
            AdjustmentAnchor { hold_ppm: SETTLEMENT_PPM, adjustment_ppm: 0 },
        ],
        ..SettlementPolicy::default()
    };
    assert!(matches!(non_monotonic.validate(), Err(DomainError::InvalidSettlementPolicy(_))));

    let legacy_mismatch = SettlementPolicy {
        version: 2,
        mode: SettlementMode::LegacyZeroV0,
        ..SettlementPolicy::default()
    };
    assert!(matches!(legacy_mismatch.validate(), Err(DomainError::InvalidSettlementPolicy(_))));

    // v1 仅供历史快照读取，新拍卖模板必须拒绝。
    assert!(matches!(
        v1_policy().validate_for_new_auction(),
        Err(DomainError::InvalidSettlementPolicy(_))
    ));
}

#[test]
fn compensation_reversal_returns_credit_before_capping_tax_at_the_balance() {
    assert_eq!(COMPENSATION_REVERSAL_RULE_VERSION, 1);

    let full = CompensationReversal::calculate(
        SettlementAmount::new(100).unwrap(),
        SettlementAmount::new(50).unwrap(),
        SettlementAmount::new(20).unwrap(),
    )
    .unwrap()
    .expect("附加费或补偿非零时必须形成冲正事实");
    assert_eq!(full.compensation_returned.value(), 20);
    assert_eq!(full.tax_reversed.value(), 50);
    assert_eq!(full.tax_shortfall.value(), 0);
    assert_eq!(full.closing_balance.value(), 70);
    assert_eq!(full.amount(), -30);

    let short = CompensationReversal::calculate(
        SettlementAmount::zero(),
        SettlementAmount::new(50).unwrap(),
        SettlementAmount::new(20).unwrap(),
    )
    .unwrap()
    .expect("池余额不足仍须形成冲正事实");
    assert_eq!(short.tax_reversed.value(), 20);
    assert_eq!(short.tax_shortfall.value(), 30);
    assert_eq!(short.closing_balance.value(), 0);
    assert_eq!(short.amount(), 0);

    assert!(
        CompensationReversal::calculate(
            SettlementAmount::new(10).unwrap(),
            SettlementAmount::zero(),
            SettlementAmount::zero(),
        )
        .unwrap()
        .is_none()
    );
}
