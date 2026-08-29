//! 竞价规划器的模块装配与稳定公共结果。

use serde::{Deserialize, Serialize};

use crate::{
    AcceptedBid, AuctionRules, AuctionStatus, BidAmount, DomainError, LotBidState,
    ProxyAuthorization, ProxyAuthorizationId, ProxyMode, ProxyStatus, ProxyTransition, UserId,
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
    /// 按旧 Public v1 的错误优先级，在锁参与者资格前校验代理策略。
    pub fn validate_proxy_creation_policy(
        rules: AuctionRules,
        auction_status: AuctionStatus,
        mode: ProxyMode,
    ) -> Result<(), DomainError> {
        proxy::validate_create_proxy_policy(rules, auction_status, mode)
    }

    /// 按旧 Public v1 的错误优先级，在锁参与者资格前校验代理重入历史。
    pub fn validate_proxy_creation_history(
        rules: AuctionRules,
        usage_number: u64,
        previous_maximum: Option<BidAmount>,
        previous_status: Option<ProxyStatus>,
        maximum_amount: BidAmount,
    ) -> Result<(), DomainError> {
        proxy::validate_create_proxy_history(
            rules,
            usage_number,
            previous_maximum,
            previous_status,
            maximum_amount,
        )
    }

    /// 在资格读取前确认 lower 命令确实指向调用者当前仍具竞价效力的授权。
    pub fn validate_lower_proxy_target(
        active: Option<ProxyAuthorization>,
        actor_id: UserId,
        authorization_id: ProxyAuthorizationId,
    ) -> Result<ProxyAuthorization, DomainError> {
        proxy::validate_lower_proxy_target(active, actor_id, authorization_id)
    }

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
