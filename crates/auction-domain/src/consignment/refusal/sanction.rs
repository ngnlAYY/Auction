//! 处罚阈值判定：main 同合同计数，isekai 买家跨合同、90 天窗口计数。

use std::collections::BTreeMap;
use std::ops::RangeInclusive;

use auction_time::Timestamp;
use uuid::Uuid;

use super::fact::{RefusalKind, ReturnFactInput};
use super::qualify::{Qualification, qualify_return};
use super::window::{IsekaiWindow, isekai_window};
use crate::UserId;

/// isekai 阈值下限（决定 18：至少 2，默认 3）。
pub const ISEKAI_MIN_THRESHOLD: u8 = 2;

/// 阈值合法范围，与 D9 §7 的配置范围一致：`cod_return_limit` 为 2..=10，
/// `isekai_refusal_limit` 为 2..=100；越界一律失败关闭，不钳制。
const MAIN_THRESHOLDS: RangeInclusive<u8> = 2..=10;
const ISEKAI_THRESHOLDS: RangeInclusive<u8> = ISEKAI_MIN_THRESHOLD..=100;

/// 处罚规则；账号理由码见 [`SanctionRule::reason_code`]。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SanctionRule {
    /// main：同一合同本周期未用的合格拒收达阈值。
    MainCodReturns,
    /// isekai：买家本周期、窗内不同合同的合格拒收达阈值。
    IsekaiUnpaidCarrierReturns,
}

impl SanctionRule {
    /// 账号处罚的理由码（已发布契约）。
    pub const fn reason_code(self) -> &'static str {
        match self {
            Self::MainCodReturns => "consignment_main_cod_returns",
            Self::IsekaiUnpaidCarrierReturns => "consignment_isekai_unpaid_carrier_returns",
        }
    }
}

/// 参加阈值判定的合格退回事实。
///
/// 字段私有，只能经 [`QualifiedRefusal::from_fact`] 构造，而它只接受资格裁决为
/// [`Qualification::Qualified`] 的事实：待判与不计的事实在类型上进不了计数池（决定 18、26）。
///
/// 两类边界情形保持现状（统筹方裁定，依据决定 26 与 D9“争议时证清前不处罚”）：
/// - 上游称可交付、退件时刻却不早于季界：矛盾输入失败关闭为待判，不处罚也不丢事实；
/// - 合同已关闭且退件时刻早于季界：上游关闭结论明确，按“交割已关闭”不计。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QualifiedRefusal {
    return_fact_id: Uuid,
    contract_id: Uuid,
    returned_at: Timestamp,
    /// 拒收种类；只与对应规则互相计数，main 与 isekai 的事实互不混入。
    kind: RefusalKind,
    /// 买家；计数池只在同一买家的事实间计数。
    buyer_user_id: UserId,
    /// 事实所属周期；解禁后旧周期的事实不再凑数（决定 23）。
    cycle_id: Uuid,
    /// 已被某项处罚用作构成项；构成项全局只用一次。
    used_by_sanction: bool,
}

impl QualifiedRefusal {
    /// 仅当事实经 [`qualify_return`] 判为合格时返回 `Some`。
    pub fn from_fact(fact: &ReturnFactInput, used_by_sanction: bool) -> Option<Self> {
        match qualify_return(fact) {
            Qualification::Qualified => Some(Self {
                return_fact_id: fact.return_fact_id,
                contract_id: fact.contract_id,
                returned_at: fact.returned_at,
                kind: fact.kind,
                buyer_user_id: fact.buyer_user_id,
                cycle_id: fact.cycle_id,
                used_by_sanction,
            }),
            Qualification::NotCounted(_) | Qualification::Pending(_) => None,
        }
    }
}

/// 一项处罚的固定判定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SanctionDecision {
    /// 触发规则。
    pub rule: SanctionRule,
    /// 判定时刻，即获证事务的锁后数据库时刻。
    pub t0: Timestamp,
    /// 冻结阈值。
    pub threshold: u8,
    /// isekai 的窗口；main 为 `None`。
    pub window: Option<IsekaiWindow>,
    /// 恰 `threshold` 个构成项，按 `(returned_at, id)` 升序，含触发事实。
    pub members: Vec<Uuid>,
}

/// 判定触发事实是否使买家达到阈值。
///
/// `pool` 之外会并入触发事实；仅计与触发事实同周期、未被处罚使用、`returned_at <= t0` 的事实。
/// isekai 另要求落在 `(t0-90d, t0]` 内（触发事实自身也须在窗内）、不同合同且每单至多一次。
/// 构成项取触发事实加其余最早的 `threshold-1` 个，结果与 `pool` 的输入顺序无关。
/// 候选与触发事实须种类对应规则、买家相同。
/// 不足阈值、阈值越界或触发事实不在计数范围时返回 `None`。
pub fn evaluate_sanction(
    rule: SanctionRule,
    trigger: &QualifiedRefusal,
    pool: &[QualifiedRefusal],
    threshold: u8,
    t0: Timestamp,
) -> Option<SanctionDecision> {
    let thresholds = match rule {
        SanctionRule::MainCodReturns => MAIN_THRESHOLDS,
        SanctionRule::IsekaiUnpaidCarrierReturns => ISEKAI_THRESHOLDS,
    };
    if !thresholds.contains(&threshold) {
        return None;
    }
    let window = match rule {
        SanctionRule::MainCodReturns => None,
        SanctionRule::IsekaiUnpaidCarrierReturns => Some(isekai_window(t0)?),
    };
    let kind = match rule {
        SanctionRule::MainCodReturns => RefusalKind::MainCodReturn,
        SanctionRule::IsekaiUnpaidCarrierReturns => RefusalKind::IsekaiUnpaidCarrierReturn,
    };
    let counts = |candidate: &QualifiedRefusal| {
        // 种类与买家隔离：他种规则或他人的事实不得凑数，也不得充当触发事实。
        candidate.kind == kind
            && candidate.buyer_user_id == trigger.buyer_user_id
            && candidate.cycle_id == trigger.cycle_id
            && !candidate.used_by_sanction
            && candidate.returned_at <= t0
            && window.is_none_or(|window| window.contains(candidate.returned_at))
    };
    if !counts(trigger) {
        return None;
    }
    let needed = usize::from(threshold) - 1;
    let others = match rule {
        SanctionRule::MainCodReturns => same_contract_others(trigger, pool, &counts),
        SanctionRule::IsekaiUnpaidCarrierReturns => {
            distinct_contract_others(trigger, pool, &counts)
        }
    };
    if others.len() < needed {
        return None;
    }
    let mut members: Vec<(Timestamp, Uuid)> = others.into_iter().take(needed).collect();
    members.push((trigger.returned_at, trigger.return_fact_id));
    members.sort();
    Some(SanctionDecision {
        rule,
        t0,
        threshold,
        window,
        members: members.into_iter().map(|(_, id)| id).collect(),
    })
}

/// 在买家互斥下判定周期内至多一项处罚：周期已有处罚时一律 `None`。
///
/// 保护的不变量：`UNIQUE(buyer_user_id, cycle_id)`，计数 2→3→4 或两个世界同时达标只形成一项。
pub fn evaluate_cycle_sanction(
    cycle_already_sanctioned: bool,
    rule: SanctionRule,
    trigger: &QualifiedRefusal,
    pool: &[QualifiedRefusal],
    threshold: u8,
    t0: Timestamp,
) -> Option<SanctionDecision> {
    if cycle_already_sanctioned {
        return None;
    }
    evaluate_sanction(rule, trigger, pool, threshold, t0)
}

/// 同合同、非触发事实本身的候选，按 `(returned_at, id)` 升序去重。
fn same_contract_others(
    trigger: &QualifiedRefusal,
    pool: &[QualifiedRefusal],
    counts: &impl Fn(&QualifiedRefusal) -> bool,
) -> Vec<(Timestamp, Uuid)> {
    let mut others: Vec<(Timestamp, Uuid)> = pool
        .iter()
        .filter(|candidate| {
            candidate.contract_id == trigger.contract_id
                && candidate.return_fact_id != trigger.return_fact_id
                && counts(candidate)
        })
        .map(|candidate| (candidate.returned_at, candidate.return_fact_id))
        .collect();
    others.sort();
    others.dedup();
    others
}

/// 不同合同的候选：每个合同取最早一份，触发事实所在合同由触发事实代表。
fn distinct_contract_others(
    trigger: &QualifiedRefusal,
    pool: &[QualifiedRefusal],
    counts: &impl Fn(&QualifiedRefusal) -> bool,
) -> Vec<(Timestamp, Uuid)> {
    let mut earliest: BTreeMap<Uuid, (Timestamp, Uuid)> = BTreeMap::new();
    for candidate in pool
        .iter()
        .filter(|candidate| candidate.contract_id != trigger.contract_id && counts(candidate))
    {
        let entry = (candidate.returned_at, candidate.return_fact_id);
        earliest
            .entry(candidate.contract_id)
            .and_modify(|current| *current = (*current).min(entry))
            .or_insert(entry);
    }
    let mut others: Vec<(Timestamp, Uuid)> = earliest.into_values().collect();
    others.sort();
    others
}
