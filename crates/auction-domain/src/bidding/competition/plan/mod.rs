//! 竞价规划器的模块装配与稳定公共结果。

use serde::{Deserialize, Serialize};

use crate::{
    AcceptedBid, AuctionRules, DomainError, LotBidState, ProxyAuthorization, ProxyAuthorizationId,
    ProxyStatus, ProxyTransition,
};

use super::trigger::{CompetitionTrigger, CompetitionTriggerKind};

mod ordinary;
mod proxy;
mod state;
mod validation;

/// 一个命令在锁内计算出的完整竞价结果。
///
/// 应用层必须把这里的公开事实、代理转换、快照和最终 lot 投影放进同一个本地事务；
/// 规划器本身不修改输入，也不执行数据库或网络 I/O。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompetitionPlan {
    pub public_bids: Vec<AcceptedBid>,
    pub proxy_transitions: Vec<ProxyTransition>,
    pub proxy_states: Vec<ProxyAuthorization>,
    pub active_proxy_id: Option<ProxyAuthorizationId>,
    pub final_lot: LotBidState,
    pub soft_close_applied: bool,
}

impl CompetitionPlan {
    /// 计算一次竞价、代理或代理生命周期命令的原子结果。
    pub fn plan(
        rules: AuctionRules,
        state: LotBidState,
        active: Option<ProxyAuthorization>,
        trigger: CompetitionTrigger,
    ) -> Result<Self, DomainError> {
        rules.validate()?;
        state.validate()?;
        validation::validate_active_authorization(state, active.as_ref())?;
        validation::validate_trigger_basics(&trigger)?;

        let plan = match trigger.kind() {
            CompetitionTriggerKind::OrdinaryBid => {
                ordinary::plan_ordinary_bid(rules, state, active, trigger)
            }
            CompetitionTriggerKind::CreateProxy => {
                proxy::plan_create_proxy(rules, state, active, trigger)
            }
            CompetitionTriggerKind::LowerProxy => {
                proxy::plan_lower_proxy(rules, state, active, trigger)
            }
            CompetitionTriggerKind::SlowDue => proxy::plan_slow_due(rules, state, active, trigger),
            CompetitionTriggerKind::FinalProxy => {
                proxy::plan_final_proxy(rules, state, active, trigger)
            }
            CompetitionTriggerKind::FreezeProxy => {
                proxy::plan_terminal_proxy(rules, state, active, trigger, ProxyStatus::Frozen)
            }
            CompetitionTriggerKind::CloseProxy => {
                proxy::plan_terminal_proxy(rules, state, active, trigger, ProxyStatus::Closed)
            }
        }?;

        plan.validate()?;
        Ok(plan)
    }
}
