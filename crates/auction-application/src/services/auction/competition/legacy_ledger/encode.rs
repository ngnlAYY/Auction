use auction_domain::CompetitionPlan;
use chrono::{DateTime, Utc};

use super::super::CompetitionCommand;
use super::EncodedCommandResult;
use super::bid::LegacyBidResult;
use super::legacy_payload_error;
use super::proxy_result::LegacyProxyResult;
use crate::errors::ApplicationError;
use crate::errors::diagnostics::{
    DiagnosticContext, DiagnosticError, DiagnosticLevel, diagnostic_id, record_error,
};
use crate::ports::core::CompetitionProxyTransitionRecord;

/// 把新计算出的计划编码为旧实例能够严格重放的成功结果。
#[tracing::instrument(
    skip_all,
    fields(
        lot_id = %command.lot_id(),
        command_id = %command.command_id(),
    )
)]
pub(in crate::services::auction::competition) fn encode(
    command: &CompetitionCommand,
    plan: &CompetitionPlan,
    previous_transitions: &[CompetitionProxyTransitionRecord],
    quota_limit: u32,
    created_at: DateTime<Utc>,
) -> Result<EncodedCommandResult, ApplicationError> {
    plan.validate().map_err(|error| {
        record_error(
            DiagnosticLevel::Error,
            error.diagnostic_kind(),
            "competition_ledger",
            "validate_encoded_plan",
            DiagnosticContext {
                lot_id: diagnostic_id(&command.lot_id()),
                command_id: diagnostic_id(&command.command_id()),
                ..Default::default()
            },
        );
        legacy_payload_error()
    })?;
    match command {
        CompetitionCommand::PlaceBid(place) => {
            let result = LegacyBidResult::from_plan(plan)?;
            if !result.validate_place_bid(place) {
                return Err(legacy_payload_error());
            }
            Ok(EncodedCommandResult { payload: serde_json::to_vec(&result)?, proxy: None })
        }
        CompetitionCommand::CreateProxy(create) => {
            let result = LegacyProxyResult::from_plan(
                command,
                plan,
                previous_transitions,
                quota_limit,
                created_at,
            )?;
            if !result.validate_create_proxy(create) {
                return Err(legacy_payload_error());
            }
            Ok(EncodedCommandResult {
                payload: serde_json::to_vec(&result)?,
                proxy: Some(result.command_outcome(plan)?),
            })
        }
        CompetitionCommand::LowerProxy(lower) => {
            let result = LegacyProxyResult::from_plan(
                command,
                plan,
                previous_transitions,
                quota_limit,
                created_at,
            )?;
            if !result.validate_lower_proxy(lower) {
                return Err(legacy_payload_error());
            }
            Ok(EncodedCommandResult {
                payload: serde_json::to_vec(&result)?,
                proxy: Some(result.command_outcome(plan)?),
            })
        }
    }
}
