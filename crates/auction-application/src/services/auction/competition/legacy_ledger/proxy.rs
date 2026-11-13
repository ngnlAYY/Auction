//! 旧版代理模式、状态与转换记录的 wire 类型。

use auction_domain::{
    BidAmount, CommandId, ProxyAuthorizationId, ProxyMode, ProxyStatus, ProxyTerminalReason,
    ProxyTransition, ProxyTransitionKind,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::legacy_payload_error;
use crate::errors::ApplicationError;
use crate::errors::diagnostics::{
    DiagnosticContext, DiagnosticError, DiagnosticLevel, record_error,
};
use crate::ports::auction::{CompetitionProxyTransitionOutcome, CompetitionWireTimestamp};
use crate::ports::core::CompetitionProxyTransitionRecord;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum LegacyProxyMode {
    Fast,
    Slow,
}

impl LegacyProxyMode {
    pub(super) const fn from_domain(value: ProxyMode) -> Self {
        match value {
            ProxyMode::Fast => Self::Fast,
            ProxyMode::Slow => Self::Slow,
        }
    }

    pub(super) const fn into_domain(self) -> ProxyMode {
        match self {
            Self::Fast => ProxyMode::Fast,
            Self::Slow => ProxyMode::Slow,
        }
    }

    pub(super) fn from_storage(value: &str) -> Option<Self> {
        match value {
            "fast" => Some(Self::Fast),
            "slow" => Some(Self::Slow),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum LegacyProxyStatus {
    Active,
    Exhausted,
    Frozen,
    Closed,
}

impl LegacyProxyStatus {
    pub(super) const fn from_domain(value: ProxyStatus) -> Self {
        match value {
            ProxyStatus::Active | ProxyStatus::FinalFast => Self::Active,
            ProxyStatus::Exhausted => Self::Exhausted,
            ProxyStatus::Frozen => Self::Frozen,
            ProxyStatus::Closed => Self::Closed,
        }
    }

    pub(super) const fn into_domain(self) -> ProxyStatus {
        match self {
            Self::Active => ProxyStatus::Active,
            Self::Exhausted => ProxyStatus::Exhausted,
            Self::Frozen => ProxyStatus::Frozen,
            Self::Closed => ProxyStatus::Closed,
        }
    }

    pub(super) fn from_storage(value: &str) -> Option<Self> {
        match value {
            "active" | "final_fast" => Some(Self::Active),
            "exhausted" => Some(Self::Exhausted),
            "frozen" => Some(Self::Frozen),
            "closed" => Some(Self::Closed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum LegacyProxyTerminalReason {
    StrictlyDefeated,
    RankedBelow,
    EligibilityLost,
    CompetitionClosed,
}

impl LegacyProxyTerminalReason {
    pub(super) const fn from_domain(value: ProxyTerminalReason) -> Self {
        match value {
            ProxyTerminalReason::StrictlyDefeated => Self::StrictlyDefeated,
            ProxyTerminalReason::RankedBelow => Self::RankedBelow,
            ProxyTerminalReason::EligibilityLost => Self::EligibilityLost,
            ProxyTerminalReason::CompetitionClosed => Self::CompetitionClosed,
        }
    }

    pub(super) const fn into_domain(self) -> ProxyTerminalReason {
        match self {
            Self::StrictlyDefeated => ProxyTerminalReason::StrictlyDefeated,
            Self::RankedBelow => ProxyTerminalReason::RankedBelow,
            Self::EligibilityLost => ProxyTerminalReason::EligibilityLost,
            Self::CompetitionClosed => ProxyTerminalReason::CompetitionClosed,
        }
    }

    pub(super) fn from_storage(value: &str) -> Option<Self> {
        match value {
            "strictly_defeated" => Some(Self::StrictlyDefeated),
            "ranked_below" => Some(Self::RankedBelow),
            "eligibility_lost" => Some(Self::EligibilityLost),
            "competition_closed" => Some(Self::CompetitionClosed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum LegacyProxyTransitionKind {
    Created,
    Lowered,
    FinalFast,
    Exhausted,
    Frozen,
    Closed,
}

impl LegacyProxyTransitionKind {
    pub(super) const fn from_domain(value: ProxyTransitionKind) -> Self {
        match value {
            ProxyTransitionKind::Created => Self::Created,
            ProxyTransitionKind::Lowered => Self::Lowered,
            ProxyTransitionKind::FinalFast => Self::FinalFast,
            ProxyTransitionKind::Exhausted => Self::Exhausted,
            ProxyTransitionKind::Frozen => Self::Frozen,
            ProxyTransitionKind::Closed => Self::Closed,
        }
    }

    pub(super) const fn into_domain(self) -> ProxyTransitionKind {
        match self {
            Self::Created => ProxyTransitionKind::Created,
            Self::Lowered => ProxyTransitionKind::Lowered,
            Self::FinalFast => ProxyTransitionKind::FinalFast,
            Self::Exhausted => ProxyTransitionKind::Exhausted,
            Self::Frozen => ProxyTransitionKind::Frozen,
            Self::Closed => ProxyTransitionKind::Closed,
        }
    }

    pub(super) fn from_storage(value: &str) -> Option<Self> {
        match value {
            "created" => Some(Self::Created),
            "lowered" => Some(Self::Lowered),
            "final_fast" => Some(Self::FinalFast),
            "exhausted" => Some(Self::Exhausted),
            "frozen" => Some(Self::Frozen),
            "closed" => Some(Self::Closed),
            _ => None,
        }
    }
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LegacyProxyTransitionRecord {
    pub(super) transition_number: i64,
    pub(super) kind: LegacyProxyTransitionKind,
    pub(super) old_maximum_amount: Option<i64>,
    pub(super) new_maximum_amount: i64,
    pub(super) old_mode: Option<LegacyProxyMode>,
    pub(super) new_mode: LegacyProxyMode,
    pub(super) old_status: Option<LegacyProxyStatus>,
    pub(super) new_status: LegacyProxyStatus,
    pub(super) reason: Option<LegacyProxyTerminalReason>,
    pub(super) resulting_state_version: i64,
    pub(super) created_at: CompetitionWireTimestamp,
}

impl LegacyProxyTransitionRecord {
    pub(super) fn from_storage(
        value: &CompetitionProxyTransitionRecord,
    ) -> Result<Self, ApplicationError> {
        Ok(Self {
            transition_number: value.transition_number,
            kind: LegacyProxyTransitionKind::from_storage(&value.kind)
                .ok_or_else(legacy_payload_error)?,
            old_maximum_amount: value.old_maximum_amount,
            new_maximum_amount: value.new_maximum_amount,
            old_mode: match value.old_mode.as_deref() {
                Some(mode) => {
                    Some(LegacyProxyMode::from_storage(mode).ok_or_else(legacy_payload_error)?)
                }
                None => None,
            },
            new_mode: LegacyProxyMode::from_storage(&value.new_mode)
                .ok_or_else(legacy_payload_error)?,
            old_status: match value.old_status.as_deref() {
                Some(status) => {
                    Some(LegacyProxyStatus::from_storage(status).ok_or_else(legacy_payload_error)?)
                }
                None => None,
            },
            new_status: LegacyProxyStatus::from_storage(&value.new_status)
                .ok_or_else(legacy_payload_error)?,
            reason: match value.reason.as_deref() {
                Some(reason) => Some(
                    LegacyProxyTerminalReason::from_storage(reason)
                        .ok_or_else(legacy_payload_error)?,
                ),
                None => None,
            },
            resulting_state_version: value.resulting_state_version,
            created_at: CompetitionWireTimestamp::from_utc(value.created_at),
        })
    }

    pub(super) fn from_domain(value: &ProxyTransition, created_at: DateTime<Utc>) -> Self {
        Self {
            transition_number: i64::try_from(value.state_version).unwrap_or(i64::MAX),
            kind: LegacyProxyTransitionKind::from_domain(value.kind),
            old_maximum_amount: value.old_maximum.map(BidAmount::value),
            new_maximum_amount: value.new_maximum.value(),
            old_mode: value.old_mode.map(LegacyProxyMode::from_domain),
            new_mode: LegacyProxyMode::from_domain(value.new_mode),
            old_status: value.old_status.map(LegacyProxyStatus::from_domain),
            new_status: LegacyProxyStatus::from_domain(value.new_status),
            reason: value.reason.map(LegacyProxyTerminalReason::from_domain),
            resulting_state_version: i64::try_from(value.state_version).unwrap_or(i64::MAX),
            created_at: CompetitionWireTimestamp::from_utc(created_at),
        }
    }

    pub(super) fn to_outcome(&self) -> Result<CompetitionProxyTransitionOutcome, ApplicationError> {
        Ok(CompetitionProxyTransitionOutcome {
            transition_number: u64::try_from(self.transition_number).map_err(|error| {
                record_error(
                    DiagnosticLevel::Error,
                    error.diagnostic_kind(),
                    "competition_ledger",
                    "decode_transition_number",
                    DiagnosticContext::default(),
                );
                legacy_payload_error()
            })?,
            kind: self.kind.into_domain(),
            old_maximum_amount: self.old_maximum_amount.map(BidAmount::new).transpose().map_err(
                |error| {
                    record_error(
                        DiagnosticLevel::Error,
                        error.diagnostic_kind(),
                        "competition_ledger",
                        "decode_previous_maximum",
                        DiagnosticContext::default(),
                    );
                    legacy_payload_error()
                },
            )?,
            new_maximum_amount: BidAmount::new(self.new_maximum_amount).map_err(|error| {
                record_error(
                    DiagnosticLevel::Error,
                    error.diagnostic_kind(),
                    "competition_ledger",
                    "decode_next_maximum",
                    DiagnosticContext::default(),
                );
                legacy_payload_error()
            })?,
            old_mode: self.old_mode.map(LegacyProxyMode::into_domain),
            new_mode: self.new_mode.into_domain(),
            old_status: self.old_status.map(LegacyProxyStatus::into_domain),
            new_status: self.new_status.into_domain(),
            reason: self.reason.map(LegacyProxyTerminalReason::into_domain),
            resulting_state_version: u64::try_from(self.resulting_state_version).map_err(
                |error| {
                    record_error(
                        DiagnosticLevel::Error,
                        error.diagnostic_kind(),
                        "competition_ledger",
                        "decode_transition_state_version",
                        DiagnosticContext::default(),
                    );
                    legacy_payload_error()
                },
            )?,
            created_at: self.created_at.clone(),
        })
    }

    pub(super) fn validate(&self) -> bool {
        if self.transition_number <= 0
            || self.new_maximum_amount <= 0
            || self.resulting_state_version != self.transition_number
            || self.created_at.instant().timestamp_nanos_opt().is_none()
        {
            return false;
        }
        let old_state = self.old_maximum_amount.is_some_and(|value| value > 0)
            && self.old_mode.is_some()
            && self.old_status.is_some();
        match self.kind {
            LegacyProxyTransitionKind::Created => {
                self.transition_number == 1
                    && self.old_maximum_amount.is_none()
                    && self.old_mode.is_none()
                    && self.old_status.is_none()
                    && self.new_status == LegacyProxyStatus::Active
                    && self.reason.is_none()
            }
            LegacyProxyTransitionKind::Lowered => {
                old_state
                    && self.new_maximum_amount < self.old_maximum_amount.unwrap_or_default()
                    && self.old_mode == Some(self.new_mode)
                    && self.old_status == Some(LegacyProxyStatus::Active)
                    && self.new_status == LegacyProxyStatus::Active
                    && self.reason.is_none()
            }
            LegacyProxyTransitionKind::FinalFast => {
                old_state
                    && self.new_maximum_amount == self.old_maximum_amount.unwrap_or_default()
                    && self.old_mode == Some(LegacyProxyMode::Slow)
                    && self.new_mode == LegacyProxyMode::Fast
                    && self.old_status == Some(LegacyProxyStatus::Active)
                    && self.new_status == LegacyProxyStatus::Active
                    && self.reason.is_none()
            }
            LegacyProxyTransitionKind::Exhausted => {
                old_state
                    && self.new_maximum_amount == self.old_maximum_amount.unwrap_or_default()
                    && self.old_mode == Some(self.new_mode)
                    && self.old_status == Some(LegacyProxyStatus::Active)
                    && self.new_status == LegacyProxyStatus::Exhausted
                    && matches!(
                        self.reason,
                        Some(
                            LegacyProxyTerminalReason::StrictlyDefeated
                                | LegacyProxyTerminalReason::RankedBelow
                        )
                    )
            }
            LegacyProxyTransitionKind::Frozen => {
                old_state
                    && self.new_maximum_amount == self.old_maximum_amount.unwrap_or_default()
                    && self.old_mode == Some(self.new_mode)
                    && self.old_status == Some(LegacyProxyStatus::Active)
                    && self.new_status == LegacyProxyStatus::Frozen
                    && self.reason == Some(LegacyProxyTerminalReason::EligibilityLost)
            }
            LegacyProxyTransitionKind::Closed => {
                old_state
                    && self.new_maximum_amount == self.old_maximum_amount.unwrap_or_default()
                    && self.old_mode == Some(self.new_mode)
                    && self.old_status == Some(LegacyProxyStatus::Active)
                    && self.new_status == LegacyProxyStatus::Closed
                    && self.reason == Some(LegacyProxyTerminalReason::CompetitionClosed)
            }
        }
    }

    pub(super) fn to_domain(
        &self,
        command_id: CommandId,
    ) -> Result<ProxyTransition, ApplicationError> {
        let transition = ProxyTransition {
            authorization_id: ProxyAuthorizationId::from_uuid(Uuid::nil()),
            command_id,
            kind: self.kind.into_domain(),
            old_maximum: self.old_maximum_amount.map(BidAmount::new).transpose().map_err(
                |error| {
                    record_error(
                        DiagnosticLevel::Error,
                        error.diagnostic_kind(),
                        "competition_ledger",
                        "decode_domain_previous_maximum",
                        DiagnosticContext::default(),
                    );
                    legacy_payload_error()
                },
            )?,
            new_maximum: BidAmount::new(self.new_maximum_amount).map_err(|error| {
                record_error(
                    DiagnosticLevel::Error,
                    error.diagnostic_kind(),
                    "competition_ledger",
                    "decode_domain_next_maximum",
                    DiagnosticContext::default(),
                );
                legacy_payload_error()
            })?,
            old_mode: self.old_mode.map(LegacyProxyMode::into_domain),
            new_mode: self.new_mode.into_domain(),
            old_status: self.old_status.map(LegacyProxyStatus::into_domain),
            new_status: self.new_status.into_domain(),
            state_version: u64::try_from(self.resulting_state_version).map_err(|error| {
                record_error(
                    DiagnosticLevel::Error,
                    error.diagnostic_kind(),
                    "competition_ledger",
                    "decode_domain_state_version",
                    DiagnosticContext::default(),
                );
                legacy_payload_error()
            })?,
            reason: self.reason.map(LegacyProxyTerminalReason::into_domain),
        };
        Ok(transition)
    }
}
