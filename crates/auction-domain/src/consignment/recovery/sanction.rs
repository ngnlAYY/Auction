//! 账号处罚的执行决策：第 6 期在获证事务里固定处罚，第 7 期只执行其账号效果。
//!
//! 入参刻意不含时间窗、阈值与计数字段：执行者在类型上无法重数窗口，
//! 只能按固定的处罚业务键与买家周期状态决定写、复用或收口（D9 §5.2，决定 18、23）。

use uuid::Uuid;

use crate::consignment::refusal::SanctionRule;

/// main 同合同拒收处罚的账号理由码，同 [`SanctionRule::reason_code`]。
pub const SANCTION_REASON_MAIN_COD_RETURNS: &str = "consignment_main_cod_returns";
/// isekai 未付载体拒收处罚的账号理由码，同 [`SanctionRule::reason_code`]。
pub const SANCTION_REASON_ISEKAI_UNPAID_CARRIER_RETURNS: &str =
    "consignment_isekai_unpaid_carrier_returns";

/// 由账号理由码还原处罚规则；理由码是闭集，其他一律 `None`（失败关闭）。
pub fn sanction_rule_from_reason_code(code: &str) -> Option<SanctionRule> {
    match code {
        SANCTION_REASON_MAIN_COD_RETURNS => Some(SanctionRule::MainCodReturns),
        SANCTION_REASON_ISEKAI_UNPAID_CARRIER_RETURNS => {
            Some(SanctionRule::IsekaiUnpaidCarrierReturns)
        }
        _ => None,
    }
}

/// 待执行账号工作的状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SanctionWorkState {
    /// 尚未执行。
    Pending,
    /// 账号效果已落。
    Applied,
    /// 周期已解禁关闭，旧工作收口，不再禁人。
    CycleClosed,
}

/// 账号当前状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserStatusFact {
    /// 正常。
    Active,
    /// 已在黑名单。
    Blacklisted,
}

/// 执行决策的锁内事实；没有任何时间窗、阈值或计数字段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SanctionExecutionFacts {
    /// 工作状态。
    pub work_state: SanctionWorkState,
    /// 原 `(buyer, cycle)` 仍是买家的有效周期（未被解禁关闭）。
    pub cycle_still_open: bool,
    /// 已有的账号效果回执。
    pub effect_exists: Option<Uuid>,
    /// 账号当前状态。
    pub user_status: UserStatusFact,
}

/// 执行决定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SanctionExecution {
    /// 写拉黑与代理冻结、会话撤销副作用，使用原业务键。
    Apply,
    /// 已有效果：只返回原回执，不再写。
    ReuseEffect(Uuid),
    /// 同周期账号已被禁：只登记工作完成，不重复记黑名单历史。
    AlreadyBlacklistedRecordOnly,
    /// 周期已解禁关闭：工作收口为 `cycle_closed`，不禁人。
    CycleClosed,
    /// 工作已是 `Applied` 却没有账号效果回执：不可能的状态。调用方必须失败关闭并转人工对账，
    /// 不得再次执行账号效果，也不得静默补写回执（不变量 6）。
    Inconsistent,
}

/// 决定如何执行一项已固定的处罚。
///
/// 保护的不变量：旧周期的处罚不得在解禁后再禁人（决定 23）；已落效果只重放原回执，
/// 账号效果对同一处罚至多一次。`CycleClosed` 工作是终态，永不写；`Applied` 却无回执则
/// 返回 [`SanctionExecution::Inconsistent`]，失败关闭。
pub fn decide_sanction_execution(facts: &SanctionExecutionFacts) -> SanctionExecution {
    if facts.work_state == SanctionWorkState::CycleClosed {
        return SanctionExecution::CycleClosed;
    }
    if let Some(effect) = facts.effect_exists {
        return SanctionExecution::ReuseEffect(effect);
    }
    if facts.work_state == SanctionWorkState::Applied {
        return SanctionExecution::Inconsistent;
    }
    if !facts.cycle_still_open {
        return SanctionExecution::CycleClosed;
    }
    match facts.user_status {
        UserStatusFact::Blacklisted => SanctionExecution::AlreadyBlacklistedRecordOnly,
        UserStatusFact::Active => SanctionExecution::Apply,
    }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::{
        SANCTION_REASON_ISEKAI_UNPAID_CARRIER_RETURNS, SANCTION_REASON_MAIN_COD_RETURNS,
        SanctionExecution, SanctionExecutionFacts, SanctionWorkState, UserStatusFact,
        decide_sanction_execution, sanction_rule_from_reason_code,
    };
    use crate::consignment::refusal::SanctionRule;

    fn pending() -> SanctionExecutionFacts {
        SanctionExecutionFacts {
            work_state: SanctionWorkState::Pending,
            cycle_still_open: true,
            effect_exists: None,
            user_status: UserStatusFact::Active,
        }
    }

    #[test]
    fn reason_codes_are_a_closed_set_matching_the_rules() {
        for rule in [SanctionRule::MainCodReturns, SanctionRule::IsekaiUnpaidCarrierReturns] {
            assert_eq!(sanction_rule_from_reason_code(rule.reason_code()), Some(rule));
        }
        assert_eq!(SanctionRule::MainCodReturns.reason_code(), SANCTION_REASON_MAIN_COD_RETURNS);
        assert_eq!(
            SanctionRule::IsekaiUnpaidCarrierReturns.reason_code(),
            SANCTION_REASON_ISEKAI_UNPAID_CARRIER_RETURNS
        );
        for other in ["", "consignment_", "consignment_main_cod_returns ", "连续三次退回COD邮件"]
        {
            assert_eq!(sanction_rule_from_reason_code(other), None, "{other:?}");
        }
    }

    #[test]
    fn open_cycle_pending_work_applies_once_account_is_active() {
        assert_eq!(decide_sanction_execution(&pending()), SanctionExecution::Apply);
    }

    #[test]
    fn existing_effect_is_replayed_even_after_the_cycle_was_closed() {
        let effect = Uuid::from_u128(7);
        let facts = SanctionExecutionFacts {
            work_state: SanctionWorkState::Applied,
            cycle_still_open: false,
            effect_exists: Some(effect),
            user_status: UserStatusFact::Active,
        };
        assert_eq!(decide_sanction_execution(&facts), SanctionExecution::ReuseEffect(effect));
    }

    #[test]
    fn unbanned_old_cycle_never_blacklists_again() {
        // 解禁开启新周期（决定 23）：旧周期的待执行工作只收口，不再禁人。
        let facts = SanctionExecutionFacts { cycle_still_open: false, ..pending() };
        assert_eq!(decide_sanction_execution(&facts), SanctionExecution::CycleClosed);
        let closed =
            SanctionExecutionFacts { work_state: SanctionWorkState::CycleClosed, ..pending() };
        assert_eq!(decide_sanction_execution(&closed), SanctionExecution::CycleClosed);
    }

    #[test]
    fn applied_work_without_effect_receipt_fails_closed() {
        for cycle_still_open in [true, false] {
            let facts = SanctionExecutionFacts {
                work_state: SanctionWorkState::Applied,
                cycle_still_open,
                ..pending()
            };
            assert_eq!(decide_sanction_execution(&facts), SanctionExecution::Inconsistent);
        }
    }

    #[test]
    fn same_cycle_already_blacklisted_records_without_second_history() {
        let facts =
            SanctionExecutionFacts { user_status: UserStatusFact::Blacklisted, ..pending() };
        assert_eq!(
            decide_sanction_execution(&facts),
            SanctionExecution::AlreadyBlacklistedRecordOnly
        );
    }
}
