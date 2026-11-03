//! 慢速代理账本结果与原命令的只读绑定校验。
//! 重放不能受当前资格、生命周期或新金额上限影响；只核对历史计划能证明的命令、授权与世代，
//! 不重新执行竞价，也不要求未变更的授权快照出现在旧计划中。

use auction_domain::{
    BidMechanism, CompetitionPlan, ProxyMode, ProxyStatus, ProxyTerminalReason, ProxyTransitionKind,
};

use super::legacy_ledger::deterministic_id;
use crate::diagnostics::{DiagnosticContext, DiagnosticError, DiagnosticLevel, record_error};
use crate::{ApplicationError, CompetitionTransactionError, ExecuteSlowProxyCommand};

/// 接受代理反超、资格冻结或无法严格反超时的耗尽，拒绝属于其他命令的计划。
pub(super) fn validate(
    command: &ExecuteSlowProxyCommand,
    plan: &CompetitionPlan,
) -> Result<(), ApplicationError> {
    plan.validate().map_err(|error| {
        record_error(
            DiagnosticLevel::Warn,
            error.diagnostic_kind(),
            "slow_proxy",
            "validate_replay_plan",
            DiagnosticContext::default(),
        );
        invalid_result()
    })?;
    let next_generation = command.generation.checked_add(1).ok_or_else(invalid_result)?;
    if plan.final_lot.lot_id != command.lot_id
        || plan.final_lot.slow_generation != next_generation
        || plan.final_lot.slow_authorization_id.is_some()
        || plan.final_lot.slow_due_at.is_some()
        || plan.final_lot.effective_end_at <= command.due_at
    {
        return Err(invalid_result());
    }

    let valid = match plan.public_bids.as_slice() {
        [bid] => {
            bid.command_id == command.command_id
                && bid.id.as_uuid() == deterministic_id(command.command_id, "auction-bid-fact", 1)
                && bid.mechanism == BidMechanism::Proxy
                && plan.active_proxy_id == Some(command.authorization_id)
                && plan.proxy_transitions.is_empty()
                && plan.proxy_states.is_empty()
        }
        [] => {
            // 无竞价的终态必须留下当前授权的唯一转换；不能把无效果计划当成已执行。
            match (plan.proxy_transitions.as_slice(), plan.proxy_states.as_slice()) {
                ([transition], [state]) => {
                    plan.active_proxy_id.is_none()
                        && state.id == command.authorization_id
                        && transition.authorization_id == command.authorization_id
                        && transition.command_id == command.command_id
                        && matches!(
                            (transition.kind, transition.reason),
                            (
                                ProxyTransitionKind::Frozen,
                                Some(ProxyTerminalReason::EligibilityLost)
                            ) | (
                                ProxyTransitionKind::Exhausted,
                                Some(ProxyTerminalReason::StrictlyDefeated)
                            )
                        )
                        && transition.old_status == Some(ProxyStatus::Active)
                        && transition.old_mode == Some(ProxyMode::Slow)
                }
                _ => false,
            }
        }
        _ => false,
    };
    if !valid {
        return Err(invalid_result());
    }
    Ok(())
}

fn invalid_result() -> ApplicationError {
    CompetitionTransactionError::Failed("慢速代理命令账本结果与请求不一致".to_owned()).into()
}
