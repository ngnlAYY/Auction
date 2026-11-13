//! 为已接受的新版结果补回旧代理响应投影。

use auction_domain::{CompetitionPlan, ProxyAuthorizationId};

use super::super::CompetitionCommand;
use super::bid::LegacyBidResult;
use super::ids::deterministic_id;
use super::legacy_payload_error;
use super::proxy::LegacyProxyTransitionRecord;
use super::proxy_result::LegacyProxyResult;
use crate::errors::ApplicationError;
use crate::errors::diagnostics::{
    DiagnosticContext, DiagnosticError, DiagnosticLevel, diagnostic_id, record_error,
};
use crate::ports::auction::{CompetitionCommandOutcome, CompetitionWireTimestamp};
use crate::ports::core::CompetitionProxyTransitionRecord;

/// 为历史新版 `accepted + CompetitionPlan` 行在同一只读事务内补回旧代理响应投影。
#[tracing::instrument(
    skip_all,
    fields(
        lot_id = %command.lot_id(),
        command_id = %command.command_id(),
    )
)]
pub(in crate::services::auction::competition) fn replay_accepted(
    command: &CompetitionCommand,
    plan: CompetitionPlan,
    persisted_transitions: &[CompetitionProxyTransitionRecord],
    quota_limit: u32,
) -> Result<CompetitionCommandOutcome, ApplicationError> {
    plan.validate().map_err(|error| {
        record_error(
            DiagnosticLevel::Error,
            error.diagnostic_kind(),
            "competition_ledger",
            "validate_accepted_plan",
            DiagnosticContext {
                lot_id: diagnostic_id(&command.lot_id()),
                command_id: diagnostic_id(&command.command_id()),
                ..Default::default()
            },
        );
        legacy_payload_error()
    })?;
    // 内部自洽不等于属于当前请求；局部计划仍须绑定命令、lot 与请求金额/身份。
    if plan.final_lot.lot_id != command.lot_id()
        || plan.public_bids.iter().any(|bid| bid.command_id != command.command_id())
        || plan
            .proxy_transitions
            .iter()
            .any(|transition| transition.command_id != command.command_id())
    {
        return Err(legacy_payload_error());
    }
    if let CompetitionCommand::PlaceBid(place) = command {
        if !LegacyBidResult::from_plan(&plan)?.validate_place_bid(place) {
            return Err(legacy_payload_error());
        }
        let effective_end_at = CompetitionWireTimestamp::from_utc(plan.final_lot.effective_end_at);
        return Ok(CompetitionCommandOutcome { plan, proxy: None, effective_end_at });
    }
    let expected_id = match command {
        CompetitionCommand::CreateProxy(create) => ProxyAuthorizationId::from_uuid(
            deterministic_id(create.command_id, "auction-proxy-authorization", 1),
        ),
        CompetitionCommand::LowerProxy(lower) => lower.authorization_id,
        CompetitionCommand::PlaceBid(_) => return Err(legacy_payload_error()),
    };
    let state = plan
        .proxy_states
        .iter()
        .find(|state| state.id == expected_id)
        .ok_or_else(legacy_payload_error)?;
    let state_version = usize::try_from(state.state_version).map_err(|error| {
        record_error(
            DiagnosticLevel::Error,
            error.diagnostic_kind(),
            "competition_ledger",
            "convert_replay_state_version",
            DiagnosticContext {
                lot_id: diagnostic_id(&command.lot_id()),
                command_id: diagnostic_id(&command.command_id()),
                ..Default::default()
            },
        );
        legacy_payload_error()
    })?;
    let current_count = plan
        .proxy_transitions
        .iter()
        .filter(|transition| transition.authorization_id == expected_id)
        .count();
    let previous_count =
        state_version.checked_sub(current_count).ok_or_else(legacy_payload_error)?;
    if current_count == 0 || persisted_transitions.len() < state_version {
        return Err(legacy_payload_error());
    }
    let created_at = persisted_transitions
        .get(previous_count)
        .map(|transition| transition.created_at)
        .ok_or_else(legacy_payload_error)?;
    let result = LegacyProxyResult::from_plan(
        command,
        &plan,
        &persisted_transitions[..previous_count],
        quota_limit,
        created_at,
    )?;
    let persisted = persisted_transitions[..state_version]
        .iter()
        .map(LegacyProxyTransitionRecord::from_storage)
        .collect::<Result<Vec<_>, _>>()?;
    if result.transitions != persisted
        || match command {
            CompetitionCommand::CreateProxy(create) => !result.validate_create_proxy(create),
            CompetitionCommand::LowerProxy(lower) => !result.validate_lower_proxy(lower),
            CompetitionCommand::PlaceBid(_) => true,
        }
    {
        return Err(legacy_payload_error());
    }
    let proxy = Some(result.command_outcome(&plan)?);
    let effective_end_at = CompetitionWireTimestamp::from_utc(plan.final_lot.effective_end_at);
    Ok(CompetitionCommandOutcome { plan, proxy, effective_end_at })
}
