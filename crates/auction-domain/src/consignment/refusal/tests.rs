//! 拒收资格、窗口与处罚阈值的核心规则测试：每条计入 / 排除规则一正一反。

use auction_time::{Duration, Timestamp};
use uuid::Uuid;

use super::{
    Deliverability, ISEKAI_WINDOW_DAYS, NotCounted, PendingReason, Qualification, QualifiedRefusal,
    RefusalKind, ReturnFactInput, SanctionRule, evaluate_cycle_sanction, evaluate_sanction,
    isekai_window, qualify_return,
};
use crate::delivery::OriginalMailKey;
use crate::{SettlementWorld, UserId};

fn ts(day: u32, hour: u32, minute: u32) -> Timestamp {
    Timestamp::from_ymd_hms(2026, 11, day, hour, minute, 0).expect("测试时刻")
}

fn days_before(t0: Timestamp, days: i64) -> Timestamp {
    t0.checked_sub_signed(Duration::days(days)).expect("测试时刻")
}

fn id(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

const CYCLE: u128 = 900;

fn isekai_fact(returned_at: Timestamp, close: Timestamp) -> ReturnFactInput {
    ReturnFactInput {
        return_fact_id: id(1),
        k: OriginalMailKey::new("42", SettlementWorld::Isekai, "100").expect("测试原信键"),
        contract_id: id(10),
        obligation_id: id(20),
        original_round: 1,
        kind: RefusalKind::IsekaiUnpaidCarrierReturn,
        buyer_user_id: UserId::from_uuid(id(30)),
        returned_at,
        returned_minute_end: returned_at
            .checked_add_signed(Duration::minutes(1))
            .expect("测试时刻"),
        proven_at: returned_at,
        delivery_close_at: Some(close),
        cycle_id: id(CYCLE),
        deliverability: Deliverability::Deliverable,
        paid_or_opposite: false,
        asset_matches: true,
        duplicate: false,
        ownership_proven: true,
    }
}

fn main_fact() -> ReturnFactInput {
    let mut fact = isekai_fact(ts(5, 0, 0), ts(20, 0, 0));
    fact.kind = RefusalKind::MainCodReturn;
    fact.delivery_close_at = None;
    fact
}

#[test]
fn isekai_return_before_close_qualifies() {
    assert_eq!(qualify_return(&isekai_fact(ts(5, 0, 0), ts(20, 0, 0))), Qualification::Qualified);
}

#[test]
fn return_at_or_after_close_is_not_counted() {
    let close = ts(20, 0, 0);
    // 关闭等号起归季后：上游判 Closed 即不计。
    let mut at_close = isekai_fact(close, close);
    at_close.deliverability = Deliverability::Closed;
    assert_eq!(
        qualify_return(&at_close),
        Qualification::NotCounted(NotCounted::AfterDeliveryClosed)
    );
    // 上游仍称可交付但时刻已在等号：矛盾，失败关闭为待判，不处罚。
    assert_eq!(
        qualify_return(&isekai_fact(close, close)),
        Qualification::Pending(PendingReason::DeliverabilityDisputed)
    );
    // 差一分钟结束于界点：整个分钟都在界前，合格。
    assert_eq!(qualify_return(&isekai_fact(ts(19, 23, 59), close)), Qualification::Qualified);
}

#[test]
fn minute_spanning_close_is_pending_not_punished() {
    let close = ts(20, 0, 0);
    let mut fact = isekai_fact(ts(19, 23, 59), close);
    fact.returned_minute_end = close.checked_add_signed(Duration::seconds(1)).expect("测试时刻");
    assert_eq!(
        qualify_return(&fact),
        Qualification::Pending(PendingReason::BoundaryMinuteAmbiguous)
    );
}

#[test]
fn late_proof_does_not_rewrite_order() {
    // 季前已退、季后才补证：按原发生时间判，仍合格（决定 26）。
    let close = ts(20, 0, 0);
    let mut fact = isekai_fact(ts(5, 0, 0), close);
    fact.proven_at = ts(25, 0, 0);
    assert_eq!(qualify_return(&fact), Qualification::Qualified);
}

#[test]
fn disputed_or_missing_close_is_pending() {
    let mut disputed = isekai_fact(ts(5, 0, 0), ts(20, 0, 0));
    disputed.deliverability = Deliverability::Disputed;
    assert_eq!(
        qualify_return(&disputed),
        Qualification::Pending(PendingReason::DeliverabilityDisputed)
    );
    let mut no_close = isekai_fact(ts(5, 0, 0), ts(20, 0, 0));
    no_close.delivery_close_at = None;
    assert_eq!(
        qualify_return(&no_close),
        Qualification::Pending(PendingReason::DeliverabilityDisputed)
    );
}

#[test]
fn exclusions_are_not_counted_and_unproven_owner_is_pending() {
    let mut paid = main_fact();
    paid.paid_or_opposite = true;
    assert_eq!(qualify_return(&paid), Qualification::NotCounted(NotCounted::PaidOrOpposite));
    let mut wrong = main_fact();
    wrong.asset_matches = false;
    assert_eq!(qualify_return(&wrong), Qualification::NotCounted(NotCounted::WrongAsset));
    let mut duplicate = main_fact();
    duplicate.duplicate = true;
    assert_eq!(qualify_return(&duplicate), Qualification::NotCounted(NotCounted::Duplicate));
    let mut unowned = main_fact();
    unowned.ownership_proven = false;
    assert_eq!(qualify_return(&unowned), Qualification::Pending(PendingReason::OwnershipUnproven));
    // 正例：全部证据一致的 main 退回合格，且不受季界影响。
    assert_eq!(qualify_return(&main_fact()), Qualification::Qualified);
}

#[test]
fn window_is_left_open_right_closed() {
    let t0 = ts(23, 12, 0);
    let window = isekai_window(t0).expect("窗口");
    assert_eq!(ISEKAI_WINDOW_DAYS, 90);
    assert!(!window.contains(days_before(t0, 90)), "恰等于下界不计");
    assert!(
        window.contains(days_before(t0, 90).checked_add_signed(Duration::seconds(1)).expect("t"))
    );
    assert!(window.contains(t0), "恰等于上界计入");
    assert!(!window.contains(t0.checked_add_signed(Duration::seconds(1)).expect("t")));
    assert!(isekai_window(Timestamp::MIN_UTC).is_none());
}

fn cand_with(
    n: u128,
    contract: u128,
    returned_at: Timestamp,
    cycle: u128,
    used: bool,
) -> QualifiedRefusal {
    let mut fact = isekai_fact(returned_at, returned_at + Duration::days(30));
    fact.return_fact_id = id(n);
    fact.contract_id = id(contract);
    fact.cycle_id = id(cycle);
    QualifiedRefusal::from_fact(&fact, used).expect("合格事实")
}

fn cand(n: u128, contract: u128, returned_at: Timestamp) -> QualifiedRefusal {
    cand_with(n, contract, returned_at, CYCLE, false)
}

fn main_cand_with(
    n: u128,
    contract: u128,
    returned_at: Timestamp,
    cycle: u128,
    used: bool,
) -> QualifiedRefusal {
    let mut fact = main_fact();
    fact.return_fact_id = id(n);
    fact.contract_id = id(contract);
    fact.returned_at = returned_at;
    fact.returned_minute_end = returned_at + Duration::minutes(1);
    fact.proven_at = returned_at;
    fact.cycle_id = id(cycle);
    QualifiedRefusal::from_fact(&fact, used).expect("合格事实")
}

fn main_cand(n: u128, contract: u128, returned_at: Timestamp) -> QualifiedRefusal {
    main_cand_with(n, contract, returned_at, CYCLE, false)
}

fn other_buyer(mut fact: ReturnFactInput) -> ReturnFactInput {
    fact.buyer_user_id = UserId::from_uuid(id(31));
    fact
}

const ISEKAI: SanctionRule = SanctionRule::IsekaiUnpaidCarrierReturns;
const MAIN: SanctionRule = SanctionRule::MainCodReturns;

#[test]
fn isekai_three_distinct_contracts_in_window_trigger_once() {
    let t0 = ts(23, 0, 0);
    let a = cand(1, 11, days_before(t0, 89));
    let b = cand(2, 12, days_before(t0, 30));
    let c = cand(3, 13, days_before(t0, 1));
    let decision = evaluate_sanction(ISEKAI, &c, &[a, b], 3, t0).expect("达标");
    assert_eq!(decision.members, vec![id(1), id(2), id(3)]);
    assert_eq!(decision.window, isekai_window(t0));
    assert_eq!(decision.threshold, 3);
    // 反例：少一份不处罚。
    assert!(evaluate_sanction(ISEKAI, &c, &[b], 3, t0).is_none());
}

#[test]
fn isekai_window_edges_decide_membership() {
    let t0 = ts(23, 0, 0);
    let b = cand(2, 12, days_before(t0, 30));
    let c = cand(3, 13, t0);
    // 最早一份恰在 t0-90d：左开，不凑数。
    let at_edge = cand(1, 11, days_before(t0, 90));
    assert!(evaluate_sanction(ISEKAI, &c, &[at_edge, b], 3, t0).is_none());
    // 再晚一秒则入窗。
    let inside =
        cand(1, 11, days_before(t0, 90).checked_add_signed(Duration::seconds(1)).expect("t"));
    assert!(evaluate_sanction(ISEKAI, &c, &[inside, b], 3, t0).is_some());
    // 触发事实自身不在窗内（出窗或在 t0 之后）一律不触发。
    let old_trigger = cand(4, 14, days_before(t0, 91));
    assert!(evaluate_sanction(ISEKAI, &old_trigger, &[inside, b], 3, t0).is_none());
    let future = cand(5, 15, t0.checked_add_signed(Duration::seconds(1)).expect("t"));
    assert!(evaluate_sanction(ISEKAI, &future, &[inside, b], 3, t0).is_none());
}

#[test]
fn isekai_counts_each_contract_at_most_once() {
    let t0 = ts(23, 0, 0);
    let a1 = cand(1, 11, days_before(t0, 20));
    let a2 = cand(2, 11, days_before(t0, 10));
    let c = cand(3, 13, days_before(t0, 1));
    // 同一合同两份只算一次：共两单，未达 3。
    assert!(evaluate_sanction(ISEKAI, &c, &[a1, a2], 3, t0).is_none());
    // 触发事实所在合同已有未用旧事实时，由触发事实代表该合同。
    let same = cand(4, 13, days_before(t0, 40));
    let b = cand(5, 12, days_before(t0, 5));
    let decision = evaluate_sanction(ISEKAI, &c, &[same, a1, b], 3, t0).expect("达标");
    assert!(decision.members.contains(&id(3)) && !decision.members.contains(&id(4)));
}

#[test]
fn used_and_old_cycle_facts_do_not_count_again() {
    let t0 = ts(23, 0, 0);
    let a = cand(1, 11, days_before(t0, 20));
    let b = cand(2, 12, days_before(t0, 10));
    let c = cand(3, 13, days_before(t0, 1));
    assert!(evaluate_sanction(ISEKAI, &c, &[a, b], 3, t0).is_some());
    // 构成项全局只用一次。
    let used = cand_with(1, 11, days_before(t0, 20), CYCLE, true);
    assert!(evaluate_sanction(ISEKAI, &c, &[used, b], 3, t0).is_none());
    // 解禁开新周期：旧周期事实不再凑数（决定 23）。
    let old_cycle = cand_with(1, 11, days_before(t0, 20), CYCLE + 1, false);
    assert!(evaluate_sanction(ISEKAI, &c, &[old_cycle, b], 3, t0).is_none());
}

#[test]
fn two_to_four_or_both_worlds_form_one_sanction() {
    let t0 = ts(23, 0, 0);
    let a = cand(1, 11, days_before(t0, 20));
    let b = cand(2, 12, days_before(t0, 10));
    let c = cand(3, 13, days_before(t0, 5));
    let d = cand(4, 14, days_before(t0, 1));
    let first = evaluate_cycle_sanction(false, ISEKAI, &c, &[a, b], 3, t0).expect("第一项");
    // 周期已有处罚：第四份再来不再形成第二项。
    assert!(evaluate_cycle_sanction(true, ISEKAI, &d, &[a, b, c], 3, t0).is_none());
    // 构成项被占用后，第四份本身也凑不成第二项。
    let used = [
        cand_with(1, 11, days_before(t0, 20), CYCLE, first.members.contains(&id(1))),
        cand_with(2, 12, days_before(t0, 10), CYCLE, first.members.contains(&id(2))),
        cand_with(3, 13, days_before(t0, 5), CYCLE, first.members.contains(&id(3))),
    ];
    assert!(evaluate_sanction(ISEKAI, &d, &used, 3, t0).is_none());
    // 两世界各自达标：周期互斥下只形成一项，另一世界被拦。
    let m1 = main_cand(11, 21, days_before(t0, 20));
    let m2 = main_cand(12, 21, days_before(t0, 10));
    let m3 = main_cand(13, 21, days_before(t0, 5));
    assert!(evaluate_sanction(MAIN, &m3, &[m1, m2], 3, t0).is_some());
    assert!(evaluate_sanction(ISEKAI, &c, &[a, b], 3, t0).is_some());
    let main_first =
        evaluate_cycle_sanction(false, MAIN, &m3, &[m1, m2], 3, t0).expect("main 先达标");
    assert_eq!(main_first.rule, MAIN);
    assert!(evaluate_cycle_sanction(true, ISEKAI, &c, &[a, b], 3, t0).is_none());
    let isekai_first = evaluate_cycle_sanction(false, ISEKAI, &c, &[a, b], 3, t0).expect("isekai");
    assert_eq!(isekai_first.rule, ISEKAI);
    assert!(evaluate_cycle_sanction(true, MAIN, &m3, &[m1, m2], 3, t0).is_none());
}

#[test]
fn main_counts_same_contract_only() {
    let t0 = ts(23, 0, 0);
    let a = main_cand(1, 11, days_before(t0, 200));
    let b = main_cand(2, 11, days_before(t0, 100));
    let c = main_cand(3, 11, days_before(t0, 1));
    // 同合同三次，不受 90 天窗限制。
    let decision = evaluate_sanction(MAIN, &c, &[a, b], 3, t0).expect("达标");
    assert_eq!(decision.members, vec![id(1), id(2), id(3)]);
    assert_eq!(decision.window, None);
    // 跨合同不凑数。
    let other = main_cand(4, 12, days_before(t0, 50));
    assert!(evaluate_sanction(MAIN, &c, &[a, other], 3, t0).is_none());
}

#[test]
fn members_are_exactly_threshold_and_independent_of_pool_order() {
    let t0 = ts(23, 0, 0);
    let facts = [
        cand(1, 11, days_before(t0, 40)),
        cand(2, 12, days_before(t0, 30)),
        cand(3, 13, days_before(t0, 20)),
    ];
    let trigger = cand(9, 19, days_before(t0, 1));
    let forward = evaluate_sanction(ISEKAI, &trigger, &facts, 3, t0).expect("达标");
    let mut reversed = facts;
    reversed.reverse();
    let backward = evaluate_sanction(ISEKAI, &trigger, &reversed, 3, t0).expect("达标");
    assert_eq!(forward, backward);
    assert_eq!(forward.members, vec![id(1), id(2), id(9)]);
}

#[test]
fn threshold_floor_fails_closed() {
    let t0 = ts(23, 0, 0);
    let c = cand(3, 13, days_before(t0, 1));
    let mc = main_cand(3, 13, days_before(t0, 1));
    // isekai 阈值至少 2：配置成 1 或 0 不得退化成首次即罚。
    assert!(evaluate_sanction(ISEKAI, &c, &[], 1, t0).is_none());
    assert!(evaluate_sanction(ISEKAI, &c, &[], 0, t0).is_none());
    // main 阈值范围 2..=10（D9 §7）：1 与 0 同样失败关闭。
    assert!(evaluate_sanction(MAIN, &mc, &[], 1, t0).is_none());
    assert!(evaluate_sanction(MAIN, &mc, &[], 0, t0).is_none());
    // 上界：main 11、isekai 101 越界即失败关闭，即便构成项足够。
    let many: Vec<QualifiedRefusal> =
        (0..120).map(|n| cand(100 + n, 200 + n, days_before(t0, 2))).collect();
    let same_contract: Vec<QualifiedRefusal> =
        (0..20).map(|n| main_cand(300 + n, 13, days_before(t0, 2))).collect();
    assert!(evaluate_sanction(MAIN, &mc, &same_contract, 10, t0).is_some());
    assert!(evaluate_sanction(MAIN, &mc, &same_contract, 11, t0).is_none());
    assert!(evaluate_sanction(ISEKAI, &c, &many, 101, t0).is_none());
    assert!(evaluate_sanction(ISEKAI, &c, &many, 100, t0).is_some());
    let b = cand(2, 12, days_before(t0, 2));
    assert!(evaluate_sanction(ISEKAI, &c, &[b], 2, t0).is_some());
}

#[test]
fn reason_codes_are_published_contract() {
    assert_eq!(MAIN.reason_code(), "consignment_main_cod_returns");
    assert_eq!(ISEKAI.reason_code(), "consignment_isekai_unpaid_carrier_returns");
}

#[test]
fn main_future_return_does_not_count() {
    let t0 = ts(23, 0, 0);
    let a = main_cand(1, 11, days_before(t0, 20));
    let b = main_cand(2, 11, days_before(t0, 10));
    let c = main_cand(3, 11, days_before(t0, 1));
    assert!(evaluate_sanction(MAIN, &c, &[a, b], 3, t0).is_some());
    // main 没有窗口，`returned_at <= t0` 是唯一挡住未来时刻事实凑数的子句。
    let future = main_cand(4, 11, t0 + Duration::seconds(1));
    assert!(evaluate_sanction(MAIN, &c, &[a, future], 3, t0).is_none());
    assert!(evaluate_sanction(MAIN, &future, &[a, b], 3, t0).is_none());
}

#[test]
fn only_qualified_facts_enter_the_counting_pool() {
    let t0 = ts(23, 0, 0);
    let close = ts(30, 0, 0);
    let at = days_before(t0, 5);
    assert!(QualifiedRefusal::from_fact(&isekai_fact(at, close), false).is_some());
    // 待判：买家归属未证明、可交付性有争议。
    let mut unowned = isekai_fact(at, close);
    unowned.ownership_proven = false;
    assert!(QualifiedRefusal::from_fact(&unowned, false).is_none());
    let mut disputed = isekai_fact(at, close);
    disputed.deliverability = Deliverability::Disputed;
    assert!(QualifiedRefusal::from_fact(&disputed, false).is_none());
    // 不计：重复、已付、资产不符、交割已关闭。
    let mut duplicate = isekai_fact(at, close);
    duplicate.duplicate = true;
    let mut paid = isekai_fact(at, close);
    paid.paid_or_opposite = true;
    let mut wrong = isekai_fact(at, close);
    wrong.asset_matches = false;
    let mut closed = isekai_fact(at, close);
    closed.deliverability = Deliverability::Closed;
    for fact in [duplicate, paid, wrong, closed] {
        assert!(QualifiedRefusal::from_fact(&fact, false).is_none());
    }
    // 组合：两份合格加一份待判，时间与周期都对也凑不满阈值 3；换成合格事实则达标。
    let a = cand(1, 11, days_before(t0, 20));
    let b = cand(2, 12, days_before(t0, 10));
    let mut pending = isekai_fact(days_before(t0, 1), close);
    pending.return_fact_id = id(3);
    pending.contract_id = id(13);
    pending.ownership_proven = false;
    assert!(QualifiedRefusal::from_fact(&pending, false).is_none());
    let trigger = cand(3, 13, days_before(t0, 1));
    assert!(evaluate_sanction(ISEKAI, &trigger, &[a], 3, t0).is_none());
    assert!(evaluate_sanction(ISEKAI, &trigger, &[a, b], 3, t0).is_some());
}

#[test]
fn kinds_do_not_mix_between_rules() {
    let t0 = ts(23, 0, 0);
    let m1 = main_cand(1, 11, days_before(t0, 20));
    let m2 = main_cand(2, 11, days_before(t0, 10));
    let m3 = main_cand(3, 11, days_before(t0, 5));
    let i1 = cand(4, 11, days_before(t0, 20));
    let i2 = cand(5, 12, days_before(t0, 10));
    let i3 = cand(6, 13, days_before(t0, 5));
    // isekai 事实作 main 触发事实：不处罚，即便池中有足够的 main 事实。
    assert!(evaluate_sanction(MAIN, &i3, &[m1, m2], 3, t0).is_none());
    // isekai 事实混入 main 池：不计数。
    let same_contract_isekai = cand(7, 11, days_before(t0, 8));
    assert!(evaluate_sanction(MAIN, &m3, &[m1, same_contract_isekai], 3, t0).is_none());
    assert!(evaluate_sanction(MAIN, &m3, &[m1, m2], 3, t0).is_some());
    // main 事实作 isekai 触发事实或混入 isekai 池：同样不计数。
    let m_other = main_cand(8, 12, days_before(t0, 10));
    assert!(evaluate_sanction(ISEKAI, &m3, &[i1, i2], 3, t0).is_none());
    assert!(evaluate_sanction(ISEKAI, &i3, &[i1, m_other], 3, t0).is_none());
    assert!(evaluate_sanction(ISEKAI, &i3, &[i1, i2], 3, t0).is_some());
}

#[test]
fn other_buyers_facts_do_not_count() {
    let t0 = ts(23, 0, 0);
    let foreign = |n: u128, contract: u128, days: i64, main: bool| {
        let at = days_before(t0, days);
        let mut fact = if main { main_fact() } else { isekai_fact(at, at + Duration::days(30)) };
        fact.return_fact_id = id(n);
        fact.contract_id = id(contract);
        fact.returned_at = at;
        fact.returned_minute_end = at + Duration::minutes(1);
        fact.proven_at = at;
        QualifiedRefusal::from_fact(&other_buyer(fact), false).expect("合格事实")
    };
    // isekai：他人买家的事实不凑数，也不能充当触发事实。
    let a = cand(1, 11, days_before(t0, 20));
    let c = cand(3, 13, days_before(t0, 1));
    let foreign_b = foreign(2, 12, 10, false);
    assert!(evaluate_sanction(ISEKAI, &c, &[a, foreign_b], 3, t0).is_none());
    let foreign_trigger = foreign(9, 19, 1, false);
    let b = cand(2, 12, days_before(t0, 10));
    assert!(evaluate_sanction(ISEKAI, &foreign_trigger, &[a, b], 3, t0).is_none());
    // main：同合同他人买家的事实不凑数。
    let m1 = main_cand(11, 21, days_before(t0, 20));
    let m3 = main_cand(13, 21, days_before(t0, 5));
    let foreign_m2 = foreign(12, 21, 10, true);
    assert!(evaluate_sanction(MAIN, &m3, &[m1, foreign_m2], 3, t0).is_none());
    assert!(evaluate_sanction(MAIN, &foreign_m2, &[m1, m3], 3, t0).is_none());
}
