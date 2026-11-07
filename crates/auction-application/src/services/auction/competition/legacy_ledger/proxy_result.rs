use auction_domain::{
    BidAmount, CompetitionPlan, LotBidState, LotId, ProxyAuthorization, ProxyAuthorizationId,
    ProxyStatus, UserId,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use uuid::Uuid;

use super::super::CompetitionCommand;
use super::bid::{LegacyAcceptedBidTransition, LegacyBidMechanism, LegacyBidResult};
use super::ids::deterministic_id;
use super::legacy_payload_error;
use super::proxy::{
    LegacyProxyMode, LegacyProxyStatus, LegacyProxyTerminalReason, LegacyProxyTransitionKind,
    LegacyProxyTransitionRecord,
};
use crate::errors::ApplicationError;
use crate::errors::diagnostics::{
    DiagnosticContext, DiagnosticError, DiagnosticLevel, diagnostic_id, record_error,
};
use crate::ports::auction::{CompetitionProxyCommandOutcome, CompetitionWireTimestamp};
use crate::ports::core::CompetitionProxyTransitionRecord;

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LegacyProxyResult {
    authorization_id: [u8; 16],
    lot_id: [u8; 16],
    user_id: [u8; 16],
    authorization_sequence: i64,
    usage_number: i64,
    quota_used: i64,
    quota_limit: i64,
    mode: LegacyProxyMode,
    maximum_amount: i64,
    status: LegacyProxyStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    terminal_reason: Option<LegacyProxyTerminalReason>,
    state_version: i64,
    pub(super) transitions: Vec<LegacyProxyTransitionRecord>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bid: Option<LegacyBidResult>,
    /// 旧 Go 的降低/终态结果会把 nil slice 固定编码成 `facts:null`；字段仍必须存在，且
    /// 非空时继续按严格事实数组解码，不能把 HTTP 或领域边界一起放宽。
    #[serde(
        serialize_with = "serialize_legacy_proxy_facts",
        deserialize_with = "deserialize_legacy_proxy_facts"
    )]
    facts: Vec<LegacyAcceptedBidTransition>,
}

pub(super) fn serialize_legacy_proxy_facts<S>(
    facts: &Vec<LegacyAcceptedBidTransition>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    if facts.is_empty() {
        Option::<&Vec<LegacyAcceptedBidTransition>>::None.serialize(serializer)
    } else {
        facts.serialize(serializer)
    }
}

pub(super) fn deserialize_legacy_proxy_facts<'de, D>(
    deserializer: D,
) -> Result<Vec<LegacyAcceptedBidTransition>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<Vec<LegacyAcceptedBidTransition>>::deserialize(deserializer)
        .map(Option::unwrap_or_default)
}

impl LegacyProxyResult {
    pub(super) fn response_effective_end_at(
        &self,
    ) -> Result<CompetitionWireTimestamp, ApplicationError> {
        self.bid
            .as_ref()
            .map(|bid| bid.effective_end.clone())
            .or_else(|| self.transitions.last().map(|transition| transition.created_at.clone()))
            .ok_or_else(legacy_payload_error)
    }

    pub(super) fn from_plan(
        command: &CompetitionCommand,
        plan: &CompetitionPlan,
        previous_transitions: &[CompetitionProxyTransitionRecord],
        quota_limit: u32,
        created_at: DateTime<Utc>,
    ) -> Result<Self, ApplicationError> {
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

        let mut transitions = previous_transitions
            .iter()
            .map(LegacyProxyTransitionRecord::from_storage)
            .collect::<Result<Vec<_>, _>>()?;
        transitions.extend(
            plan.proxy_transitions
                .iter()
                .filter(|transition| transition.authorization_id == state.id)
                .map(|transition| LegacyProxyTransitionRecord::from_domain(transition, created_at)),
        );
        let terminal_reason = plan
            .proxy_transitions
            .iter()
            .rev()
            .find(|transition| transition.authorization_id == state.id)
            .and_then(|transition| transition.reason)
            .map(LegacyProxyTerminalReason::from_domain);
        let bid =
            (!plan.public_bids.is_empty()).then(|| LegacyBidResult::from_plan(plan)).transpose()?;
        let facts = bid.as_ref().map_or_else(Vec::new, |bid| bid.facts.clone());
        Ok(Self {
            authorization_id: *state.id.as_uuid().as_bytes(),
            lot_id: *state.lot_id.as_uuid().as_bytes(),
            user_id: *state.user_id.as_uuid().as_bytes(),
            authorization_sequence: i64::try_from(state.authorization_sequence).map_err(
                |error| {
                    record_error(
                        DiagnosticLevel::Error,
                        error.diagnostic_kind(),
                        "competition_ledger",
                        "encode_authorization_sequence",
                        DiagnosticContext::default(),
                    );
                    legacy_payload_error()
                },
            )?,
            usage_number: i64::try_from(state.usage_number).map_err(|error| {
                record_error(
                    DiagnosticLevel::Error,
                    error.diagnostic_kind(),
                    "competition_ledger",
                    "encode_usage_number",
                    DiagnosticContext::default(),
                );
                legacy_payload_error()
            })?,
            quota_used: i64::try_from(state.usage_number).map_err(|error| {
                record_error(
                    DiagnosticLevel::Error,
                    error.diagnostic_kind(),
                    "competition_ledger",
                    "encode_quota_used",
                    DiagnosticContext::default(),
                );
                legacy_payload_error()
            })?,
            quota_limit: i64::from(quota_limit),
            mode: LegacyProxyMode::from_domain(state.mode),
            maximum_amount: state.maximum_amount.value(),
            status: LegacyProxyStatus::from_domain(state.status),
            terminal_reason,
            state_version: i64::try_from(state.state_version).map_err(|error| {
                record_error(
                    DiagnosticLevel::Error,
                    error.diagnostic_kind(),
                    "competition_ledger",
                    "encode_proxy_state_version",
                    DiagnosticContext::default(),
                );
                legacy_payload_error()
            })?,
            transitions,
            bid,
            facts,
        })
    }

    fn validate(&self, lot_id: Uuid, user_id: Uuid, command_id: Uuid) -> bool {
        if lot_id.is_nil()
            || user_id.is_nil()
            || command_id.is_nil()
            || Uuid::from_bytes(self.authorization_id).is_nil()
            || Uuid::from_bytes(self.lot_id) != lot_id
            || Uuid::from_bytes(self.user_id) != user_id
            || self.authorization_sequence <= 0
            || self.usage_number <= 0
            || self.maximum_amount <= 0
            || self.state_version <= 0
            || self.quota_used <= 0
            || self.quota_used < self.usage_number
            || self.quota_limit < 0
            || self.quota_limit > 0 && self.quota_used > self.quota_limit
            || !valid_terminal_state(self.status, self.terminal_reason)
            || !validate_transition_history(self)
        {
            return false;
        }
        if self.facts.is_empty() {
            return self.bid.is_none();
        }
        let Some(bid) = self.bid.as_ref() else { return false };
        bid.validate(lot_id, command_id) && bid.facts == self.facts
    }

    pub(super) fn validate_create_proxy(
        &self,
        command: &crate::services::auction::CreateProxyCommand,
    ) -> bool {
        self.validate(
            command.lot_id.as_uuid(),
            command.bidder_id.as_uuid(),
            command.command_id.as_uuid(),
        ) && Uuid::from_bytes(self.authorization_id)
            == deterministic_id(command.command_id, "auction-proxy-authorization", 1)
            && command.mode.is_none_or(|mode| self.mode == LegacyProxyMode::from_domain(mode))
            && self.maximum_amount == command.maximum_amount.value()
            && self.quota_used == self.usage_number
            && self.facts.len() == 1
            && self.bid.is_some()
            && Uuid::from_bytes(self.facts[0].bid_id)
                == deterministic_id(command.command_id, "auction-bid-fact", 1)
            && self.facts[0].mechanism == LegacyBidMechanism::Proxy
    }

    pub(super) fn validate_lower_proxy(
        &self,
        command: &crate::services::auction::LowerProxyCommand,
    ) -> bool {
        self.validate(
            command.lot_id.as_uuid(),
            command.bidder_id.as_uuid(),
            command.command_id.as_uuid(),
        ) && Uuid::from_bytes(self.authorization_id) == command.authorization_id.as_uuid()
            && self.maximum_amount == command.new_maximum.value()
            && self.status == LegacyProxyStatus::Active
            && self.terminal_reason.is_none()
            && self.facts.is_empty()
            && self.bid.is_none()
            && self
                .transitions
                .last()
                .is_some_and(|transition| transition.kind == LegacyProxyTransitionKind::Lowered)
    }

    /// 从已经严格校验的旧结果中冻结 HTTP 成功响应，不在事务提交后读取可变账户投影。
    pub(super) fn command_outcome(
        &self,
        plan: &CompetitionPlan,
    ) -> Result<CompetitionProxyCommandOutcome, ApplicationError> {
        let authorization_id =
            ProxyAuthorizationId::from_uuid(Uuid::from_bytes(self.authorization_id));
        let authorization = plan
            .proxy_states
            .iter()
            .find(|state| state.id == authorization_id)
            .copied()
            .ok_or_else(legacy_payload_error)?;
        authorization.validate().map_err(|error| {
            record_error(
                DiagnosticLevel::Error,
                error.diagnostic_kind(),
                "competition_ledger",
                "validate_replayed_authorization",
                DiagnosticContext { lot_id: diagnostic_id(&self.lot_id), ..Default::default() },
            );
            legacy_payload_error()
        })?;

        if authorization.lot_id.as_uuid() != Uuid::from_bytes(self.lot_id)
            || authorization.user_id.as_uuid() != Uuid::from_bytes(self.user_id)
            || i64::try_from(authorization.authorization_sequence).ok()
                != Some(self.authorization_sequence)
            || i64::try_from(authorization.usage_number).ok() != Some(self.usage_number)
            || authorization.maximum_amount.value() != self.maximum_amount
            || LegacyProxyMode::from_domain(authorization.mode) != self.mode
            || LegacyProxyStatus::from_domain(authorization.status) != self.status
            || i64::try_from(authorization.state_version).ok() != Some(self.state_version)
            || self.quota_used != self.usage_number
            || !validate_transition_history(self)
        {
            return Err(legacy_payload_error());
        }

        Ok(CompetitionProxyCommandOutcome {
            authorization,
            quota_used: u64::try_from(self.quota_used).map_err(|error| {
                record_error(
                    DiagnosticLevel::Error,
                    error.diagnostic_kind(),
                    "competition_ledger",
                    "decode_quota_used",
                    DiagnosticContext { lot_id: diagnostic_id(&self.lot_id), ..Default::default() },
                );
                legacy_payload_error()
            })?,
            quota_limit: u32::try_from(self.quota_limit).map_err(|error| {
                record_error(
                    DiagnosticLevel::Error,
                    error.diagnostic_kind(),
                    "competition_ledger",
                    "decode_quota_limit",
                    DiagnosticContext { lot_id: diagnostic_id(&self.lot_id), ..Default::default() },
                );
                legacy_payload_error()
            })?,
            transitions: self
                .transitions
                .iter()
                .map(LegacyProxyTransitionRecord::to_outcome)
                .collect::<Result<Vec<_>, _>>()?,
            terminal_reason: self.terminal_reason.map(LegacyProxyTerminalReason::into_domain),
        })
    }

    pub(super) fn to_plan(
        &self,
        command: &CompetitionCommand,
    ) -> Result<CompetitionPlan, ApplicationError> {
        let command_id = command.command_id();
        let authorization_id =
            ProxyAuthorizationId::from_uuid(Uuid::from_bytes(self.authorization_id));
        let lot_id = LotId::from_uuid(Uuid::from_bytes(self.lot_id));
        let user_id = UserId::from_uuid(Uuid::from_bytes(self.user_id));
        let status = self.status.into_domain();
        let state = ProxyAuthorization {
            id: authorization_id,
            lot_id,
            user_id,
            // 旧结果不携带创建命令；重放计划只需要一个稳定非空内部身份，不会再次持久化。
            creation_command_id: command_id,
            authorization_sequence: u64::try_from(self.authorization_sequence).map_err(
                |error| {
                    record_error(
                        DiagnosticLevel::Error,
                        error.diagnostic_kind(),
                        "competition_ledger",
                        "decode_authorization_sequence",
                        DiagnosticContext::default(),
                    );
                    legacy_payload_error()
                },
            )?,
            usage_number: u64::try_from(self.usage_number).map_err(|error| {
                record_error(
                    DiagnosticLevel::Error,
                    error.diagnostic_kind(),
                    "competition_ledger",
                    "decode_usage_number",
                    DiagnosticContext::default(),
                );
                legacy_payload_error()
            })?,
            maximum_amount: BidAmount::new(self.maximum_amount).map_err(|error| {
                record_error(
                    DiagnosticLevel::Error,
                    error.diagnostic_kind(),
                    "competition_ledger",
                    "decode_proxy_maximum",
                    DiagnosticContext::default(),
                );
                legacy_payload_error()
            })?,
            mode: self.mode.into_domain(),
            status,
            state_version: u64::try_from(self.state_version).map_err(|error| {
                record_error(
                    DiagnosticLevel::Error,
                    error.diagnostic_kind(),
                    "competition_ledger",
                    "decode_proxy_state_version",
                    DiagnosticContext::default(),
                );
                legacy_payload_error()
            })?,
        };
        state.validate().map_err(|error| {
            record_error(
                DiagnosticLevel::Error,
                error.diagnostic_kind(),
                "competition_ledger",
                "validate_proxy_state",
                DiagnosticContext::default(),
            );
            legacy_payload_error()
        })?;

        let current_records = match command {
            CompetitionCommand::CreateProxy(_) => self.transitions.as_slice(),
            CompetitionCommand::LowerProxy(_) => self
                .transitions
                .last()
                .map(std::slice::from_ref)
                .ok_or_else(legacy_payload_error)?,
            CompetitionCommand::PlaceBid(_) => return Err(legacy_payload_error()),
        };
        let mut proxy_transitions = Vec::with_capacity(current_records.len());
        for record in current_records {
            let mut transition = record.to_domain(command_id)?;
            transition.authorization_id = authorization_id;
            proxy_transitions.push(transition);
        }

        let (public_bids, final_lot) = match self.bid.as_ref() {
            Some(bid) => {
                let plan =
                    bid.to_plan(u64::try_from(self.authorization_sequence).map_err(|error| {
                        record_error(
                            DiagnosticLevel::Error,
                            error.diagnostic_kind(),
                            "competition_ledger",
                            "decode_bid_proxy_sequence",
                            DiagnosticContext::default(),
                        );
                        legacy_payload_error()
                    })?)?;
                (plan.public_bids, plan.final_lot)
            }
            None => {
                let maximum = BidAmount::new(self.maximum_amount).map_err(|error| {
                    record_error(
                        DiagnosticLevel::Error,
                        error.diagnostic_kind(),
                        "competition_ledger",
                        "decode_idle_proxy_maximum",
                        DiagnosticContext::default(),
                    );
                    legacy_payload_error()
                })?;
                let effective_end_at = self
                    .transitions
                    .last()
                    .map(|transition| transition.created_at.instant())
                    .ok_or_else(legacy_payload_error)?;
                (
                    Vec::new(),
                    LotBidState {
                        lot_id,
                        start_price: maximum,
                        current_price: None,
                        leader_id: None,
                        leader_proxy_last: false,
                        material: false,
                        effective_end_at,
                        state_version: 1,
                        last_proxy_sequence: u64::try_from(self.authorization_sequence).map_err(
                            |error| {
                                record_error(
                                    DiagnosticLevel::Error,
                                    error.diagnostic_kind(),
                                    "competition_ledger",
                                    "decode_idle_proxy_sequence",
                                    DiagnosticContext::default(),
                                );
                                legacy_payload_error()
                            },
                        )?,
                        slow_generation: 0,
                        slow_authorization_id: None,
                        slow_due_at: None,
                    },
                )
            }
        };
        let plan = CompetitionPlan {
            public_bids,
            proxy_transitions,
            proxy_states: vec![state],
            active_proxy_id: (status == ProxyStatus::Active).then_some(authorization_id),
            final_lot,
            soft_close_applied: false,
        };
        plan.validate().map_err(|error| {
            record_error(
                DiagnosticLevel::Error,
                error.diagnostic_kind(),
                "competition_ledger",
                "validate_proxy_plan",
                DiagnosticContext::default(),
            );
            legacy_payload_error()
        })?;
        Ok(plan)
    }
}

fn valid_terminal_state(
    status: LegacyProxyStatus,
    reason: Option<LegacyProxyTerminalReason>,
) -> bool {
    match status {
        LegacyProxyStatus::Active => reason.is_none(),
        LegacyProxyStatus::Exhausted => matches!(
            reason,
            Some(
                LegacyProxyTerminalReason::StrictlyDefeated
                    | LegacyProxyTerminalReason::RankedBelow
            )
        ),
        LegacyProxyStatus::Frozen => reason == Some(LegacyProxyTerminalReason::EligibilityLost),
        LegacyProxyStatus::Closed => reason == Some(LegacyProxyTerminalReason::CompetitionClosed),
    }
}

fn validate_transition_history(result: &LegacyProxyResult) -> bool {
    if result.transitions.is_empty()
        || i64::try_from(result.transitions.len()).ok() != Some(result.state_version)
    {
        return false;
    }
    for (index, record) in result.transitions.iter().enumerate() {
        let expected_number = i64::try_from(index).ok().and_then(|value| value.checked_add(1));
        if !record.validate() || Some(record.transition_number) != expected_number {
            return false;
        }
        if index > 0 {
            let previous = &result.transitions[index - 1];
            if record.created_at < previous.created_at
                || record.old_maximum_amount != Some(previous.new_maximum_amount)
                || record.old_mode != Some(previous.new_mode)
                || record.old_status != Some(previous.new_status)
            {
                return false;
            }
        }
    }
    let Some(last) = result.transitions.last() else { return false };
    last.new_maximum_amount == result.maximum_amount
        && last.new_mode == result.mode
        && last.new_status == result.status
        && last.resulting_state_version == result.state_version
        && last.reason == result.terminal_reason
}
