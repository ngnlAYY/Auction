//! 把旧版成功结果严格解码为只读重放计划。

use super::super::CompetitionCommand;
use super::bid::LegacyBidResult;
use super::legacy_payload_error;
use super::proxy_result::LegacyProxyResult;
use crate::errors::ApplicationError;
use crate::errors::diagnostics::{
    DiagnosticContext, DiagnosticError, DiagnosticLevel, diagnostic_id, record_error,
};
use crate::ports::auction::CompetitionCommandOutcome;

/// 严格读取一条旧版成功结果，并映射为只用于无副作用重放的内部计划。
#[tracing::instrument(
    skip_all,
    fields(
        lot_id = %command.lot_id(),
        command_id = %command.command_id(),
    )
)]
pub(in crate::services::auction::competition) fn replay(
    command: &CompetitionCommand,
    payload: &[u8],
) -> Result<CompetitionCommandOutcome, ApplicationError> {
    let (plan, proxy, effective_end_at) = match command {
        CompetitionCommand::PlaceBid(place) => {
            let result: LegacyBidResult = serde_json::from_slice(payload).map_err(|error| {
                record_error(
                    DiagnosticLevel::Error,
                    error.diagnostic_kind(),
                    "competition_ledger",
                    "decode_bid_result",
                    DiagnosticContext {
                        lot_id: diagnostic_id(&command.lot_id()),
                        command_id: diagnostic_id(&command.command_id()),
                        ..Default::default()
                    },
                );
                legacy_payload_error()
            })?;
            if !result.validate_place_bid(place) {
                return Err(legacy_payload_error());
            }
            let effective_end_at = result.effective_end.clone();
            (result.to_plan(0)?, None, effective_end_at)
        }
        CompetitionCommand::CreateProxy(create) => {
            let result: LegacyProxyResult = serde_json::from_slice(payload).map_err(|error| {
                record_error(
                    DiagnosticLevel::Error,
                    error.diagnostic_kind(),
                    "competition_ledger",
                    "decode_create_proxy_result",
                    DiagnosticContext {
                        lot_id: diagnostic_id(&command.lot_id()),
                        command_id: diagnostic_id(&command.command_id()),
                        ..Default::default()
                    },
                );
                legacy_payload_error()
            })?;
            if !result.validate_create_proxy(create) {
                return Err(legacy_payload_error());
            }
            let plan = result.to_plan(command)?;
            let proxy = Some(result.command_outcome(&plan)?);
            let effective_end_at = result.response_effective_end_at()?;
            (plan, proxy, effective_end_at)
        }
        CompetitionCommand::LowerProxy(lower) => {
            let result: LegacyProxyResult = serde_json::from_slice(payload).map_err(|error| {
                record_error(
                    DiagnosticLevel::Error,
                    error.diagnostic_kind(),
                    "competition_ledger",
                    "decode_lower_proxy_result",
                    DiagnosticContext {
                        lot_id: diagnostic_id(&command.lot_id()),
                        command_id: diagnostic_id(&command.command_id()),
                        ..Default::default()
                    },
                );
                legacy_payload_error()
            })?;
            if !result.validate_lower_proxy(lower) {
                return Err(legacy_payload_error());
            }
            let plan = result.to_plan(command)?;
            let proxy = Some(result.command_outcome(&plan)?);
            let effective_end_at = result.response_effective_end_at()?;
            (plan, proxy, effective_end_at)
        }
    };
    plan.validate().map_err(|error| {
        record_error(
            DiagnosticLevel::Error,
            error.diagnostic_kind(),
            "competition_ledger",
            "validate_replay_plan",
            DiagnosticContext {
                lot_id: diagnostic_id(&command.lot_id()),
                command_id: diagnostic_id(&command.command_id()),
                ..Default::default()
            },
        );
        legacy_payload_error()
    })?;
    Ok(CompetitionCommandOutcome { plan, proxy, effective_end_at })
}
