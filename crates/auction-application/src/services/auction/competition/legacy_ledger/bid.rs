//! 旧版 `BidResult` wire 的类型与领域映射。

use auction_domain::{
    AcceptedBid, BidAmount, BidId, BidMechanism, CommandId, CompetitionPlan, LotBidState, LotId,
    UserId,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::ids::deterministic_id;
use super::legacy_payload_error;
use crate::errors::ApplicationError;
use crate::errors::diagnostics::{
    DiagnosticContext, DiagnosticError, DiagnosticLevel, record_error,
};
use crate::ports::auction::CompetitionWireTimestamp;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum LegacyBidMechanism {
    Ordinary,
    Proxy,
}

impl LegacyBidMechanism {
    const fn from_domain(value: BidMechanism) -> Self {
        match value {
            BidMechanism::Ordinary => Self::Ordinary,
            BidMechanism::Proxy => Self::Proxy,
        }
    }

    const fn into_domain(self) -> BidMechanism {
        match self {
            Self::Ordinary => BidMechanism::Ordinary,
            Self::Proxy => BidMechanism::Proxy,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LegacyAcceptedBidTransition {
    pub(super) bid_id: [u8; 16],
    origin_command_id: [u8; 16],
    fact_index: i64,
    lot_id: [u8; 16],
    amount: i64,
    leader_id: [u8; 16],
    pub(super) effective_end: CompetitionWireTimestamp,
    accepted_state_version: i64,
    pub(super) mechanism: LegacyBidMechanism,
}

impl LegacyAcceptedBidTransition {
    fn from_domain(value: &AcceptedBid) -> Result<Self, ApplicationError> {
        value.validate().map_err(|error| {
            record_error(
                DiagnosticLevel::Error,
                error.diagnostic_kind(),
                "competition_ledger",
                "validate_bid_transition",
                DiagnosticContext::default(),
            );
            legacy_payload_error()
        })?;
        Ok(Self {
            bid_id: *value.id.as_uuid().as_bytes(),
            origin_command_id: *value.command_id.as_uuid().as_bytes(),
            fact_index: i64::from(value.fact_index),
            lot_id: *value.lot_id.as_uuid().as_bytes(),
            amount: value.amount.value(),
            leader_id: *value.bidder_id.as_uuid().as_bytes(),
            effective_end: CompetitionWireTimestamp::from_utc(value.effective_end_at),
            accepted_state_version: i64::try_from(value.state_version).map_err(|error| {
                record_error(
                    DiagnosticLevel::Error,
                    error.diagnostic_kind(),
                    "competition_ledger",
                    "encode_bid_state_version",
                    DiagnosticContext::default(),
                );
                legacy_payload_error()
            })?,
            mechanism: LegacyBidMechanism::from_domain(value.mechanism),
        })
    }

    pub(super) fn validate(&self, lot_id: Uuid, command_id: Uuid) -> bool {
        let bid_id = Uuid::from_bytes(self.bid_id);
        let leader_id = Uuid::from_bytes(self.leader_id);
        !bid_id.is_nil()
            && Uuid::from_bytes(self.origin_command_id) == command_id
            && self.fact_index > 0
            && Uuid::from_bytes(self.lot_id) == lot_id
            && self.amount > 0
            && !leader_id.is_nil()
            && self.effective_end.instant().timestamp_nanos_opt().is_some()
            && self.accepted_state_version > 0
    }

    fn to_domain(&self) -> Result<AcceptedBid, ApplicationError> {
        Ok(AcceptedBid {
            id: BidId::from_uuid(Uuid::from_bytes(self.bid_id)),
            command_id: CommandId::from_uuid(Uuid::from_bytes(self.origin_command_id)),
            lot_id: LotId::from_uuid(Uuid::from_bytes(self.lot_id)),
            bidder_id: UserId::from_uuid(Uuid::from_bytes(self.leader_id)),
            fact_index: u32::try_from(self.fact_index).map_err(|error| {
                record_error(
                    DiagnosticLevel::Error,
                    error.diagnostic_kind(),
                    "competition_ledger",
                    "decode_bid_fact_index",
                    DiagnosticContext::default(),
                );
                legacy_payload_error()
            })?,
            amount: BidAmount::from_persisted_positive(self.amount).map_err(|error| {
                record_error(
                    DiagnosticLevel::Error,
                    error.diagnostic_kind(),
                    "competition_ledger",
                    "decode_bid_amount",
                    DiagnosticContext::default(),
                );
                legacy_payload_error()
            })?,
            effective_end_at: self.effective_end.instant(),
            state_version: u64::try_from(self.accepted_state_version).map_err(|error| {
                record_error(
                    DiagnosticLevel::Error,
                    error.diagnostic_kind(),
                    "competition_ledger",
                    "decode_bid_state_version",
                    DiagnosticContext::default(),
                );
                legacy_payload_error()
            })?,
            mechanism: self.mechanism.into_domain(),
        })
    }
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LegacyBidResult {
    lot_id: [u8; 16],
    amount: i64,
    leader_id: [u8; 16],
    pub(super) effective_end: CompetitionWireTimestamp,
    state_version: i64,
    pub(super) facts: Vec<LegacyAcceptedBidTransition>,
}

impl LegacyBidResult {
    pub(super) fn from_plan(plan: &CompetitionPlan) -> Result<Self, ApplicationError> {
        let amount = plan.final_lot.current_price.ok_or_else(legacy_payload_error)?;
        let leader_id = plan.final_lot.leader_id.ok_or_else(legacy_payload_error)?;
        let facts = plan
            .public_bids
            .iter()
            .map(LegacyAcceptedBidTransition::from_domain)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            lot_id: *plan.final_lot.lot_id.as_uuid().as_bytes(),
            amount: amount.value(),
            leader_id: *leader_id.as_uuid().as_bytes(),
            effective_end: CompetitionWireTimestamp::from_utc(plan.final_lot.effective_end_at),
            state_version: i64::try_from(plan.final_lot.state_version).map_err(|error| {
                record_error(
                    DiagnosticLevel::Error,
                    error.diagnostic_kind(),
                    "competition_ledger",
                    "encode_lot_state_version",
                    DiagnosticContext::default(),
                );
                legacy_payload_error()
            })?,
            facts,
        })
    }

    pub(super) fn validate(&self, lot_id: Uuid, command_id: Uuid) -> bool {
        if lot_id.is_nil()
            || command_id.is_nil()
            || Uuid::from_bytes(self.lot_id) != lot_id
            || self.amount <= 0
            || Uuid::from_bytes(self.leader_id).is_nil()
            || self.effective_end.instant().timestamp_nanos_opt().is_none()
            || self.state_version <= 0
            || !(1..=2).contains(&self.facts.len())
        {
            return false;
        }
        for (index, fact) in self.facts.iter().enumerate() {
            let expected_index = i64::try_from(index).ok().and_then(|value| value.checked_add(1));
            if !fact.validate(lot_id, command_id)
                || Some(fact.fact_index) != expected_index
                || fact.effective_end != self.effective_end
                || index > 0
                    && fact.accepted_state_version
                        != self.facts[index - 1].accepted_state_version.saturating_add(1)
            {
                return false;
            }
        }
        let Some(last) = self.facts.last() else { return false };
        self.amount == last.amount
            && self.leader_id == last.leader_id
            && self.state_version == last.accepted_state_version
    }

    pub(super) fn validate_place_bid(
        &self,
        command: &crate::services::auction::PlaceBidCommand,
    ) -> bool {
        if !self.validate(command.lot_id.as_uuid(), command.command_id.as_uuid()) {
            return false;
        }
        let first = &self.facts[0];
        // 普通命令最多追加另一位用户的代理反击，不能仅凭首条事实放行损坏的第二条事实。
        Uuid::from_bytes(first.bid_id)
            == deterministic_id(command.command_id, "auction-bid-fact", 1)
            && first.amount == command.amount.value()
            && Uuid::from_bytes(first.leader_id) == command.bidder_id.as_uuid()
            && first.mechanism == LegacyBidMechanism::Ordinary
            && self.facts.get(1).is_none_or(|response| {
                Uuid::from_bytes(response.bid_id)
                    == deterministic_id(command.command_id, "auction-bid-fact", 2)
                    && response.mechanism == LegacyBidMechanism::Proxy
                    && response.leader_id != first.leader_id
            })
    }

    pub(super) fn to_plan(
        &self,
        last_proxy_sequence: u64,
    ) -> Result<CompetitionPlan, ApplicationError> {
        // 旧 BidResult 不携带 lot 原始起拍价；无副作用重放使用首条已接受事实作为安全下界，
        // 不能把可能更高的最终价反向冒充起拍价。
        let replay_start =
            self.facts.first().map(|fact| fact.amount).ok_or_else(legacy_payload_error)?;
        let start_price = BidAmount::from_persisted_positive(replay_start).map_err(|error| {
            record_error(
                DiagnosticLevel::Error,
                error.diagnostic_kind(),
                "competition_ledger",
                "decode_start_price",
                DiagnosticContext::default(),
            );
            legacy_payload_error()
        })?;
        let amount = BidAmount::from_persisted_positive(self.amount).map_err(|error| {
            record_error(
                DiagnosticLevel::Error,
                error.diagnostic_kind(),
                "competition_ledger",
                "decode_result_amount",
                DiagnosticContext::default(),
            );
            legacy_payload_error()
        })?;
        let leader_id = UserId::from_uuid(Uuid::from_bytes(self.leader_id));
        let leader_proxy_last =
            self.facts.last().is_some_and(|fact| fact.mechanism == LegacyBidMechanism::Proxy);
        let public_bids = self
            .facts
            .iter()
            .map(LegacyAcceptedBidTransition::to_domain)
            .collect::<Result<Vec<_>, _>>()?;
        let plan = CompetitionPlan {
            public_bids,
            proxy_transitions: Vec::new(),
            proxy_states: Vec::new(),
            active_proxy_id: None,
            final_lot: LotBidState {
                lot_id: LotId::from_uuid(Uuid::from_bytes(self.lot_id)),
                start_price,
                current_price: Some(amount),
                leader_id: Some(leader_id),
                leader_proxy_last,
                material: false,
                effective_end_at: self.effective_end.instant(),
                state_version: u64::try_from(self.state_version).map_err(|error| {
                    record_error(
                        DiagnosticLevel::Error,
                        error.diagnostic_kind(),
                        "competition_ledger",
                        "decode_lot_state_version",
                        DiagnosticContext::default(),
                    );
                    legacy_payload_error()
                })?,
                last_proxy_sequence,
                slow_generation: 0,
                slow_authorization_id: None,
                slow_due_at: None,
            },
            soft_close_applied: false,
        };
        plan.validate().map_err(|error| {
            record_error(
                DiagnosticLevel::Error,
                error.diagnostic_kind(),
                "competition_ledger",
                "validate_bid_plan",
                DiagnosticContext::default(),
            );
            legacy_payload_error()
        })?;
        Ok(plan)
    }
}
