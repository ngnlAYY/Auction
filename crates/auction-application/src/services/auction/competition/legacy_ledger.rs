//! Auction-BF 竞价命令结果 wire 的防腐层。
//!
//! 旧实例会严格读取 `OK + BidResult/ProxyResult`，并校验 UUID byte-array、确定性事实 ID、
//! 事实顺序和完整代理转换历史。这里集中完成旧 wire 与内部 `CompetitionPlan` 的双向映射，
//! 避免旧 DTO 扩散到领域层、HTTP 契约或存储适配器。

use auction_domain::{
    AcceptedBid, BidAmount, BidId, BidMechanism, CommandId, CompetitionPlan, LotBidState, LotId,
    ProxyAuthorization, ProxyAuthorizationId, ProxyMode, ProxyStatus, ProxyTerminalReason,
    ProxyTransition, ProxyTransitionKind, UserId,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    ApplicationError, CompetitionCommandOutcome, CompetitionProxyCommandOutcome,
    CompetitionProxyTransitionOutcome, CompetitionProxyTransitionRecord,
    CompetitionTransactionError, CompetitionWireTimestamp,
};

use super::CompetitionCommand;

/// Auction-BF `SaveCommand` 固定使用的成功结果码。
pub(super) const LEGACY_RESULT_CODE: &str = "OK";

/// 新命令写账本和返回 HTTP 时共用的同事务冻结结果。
pub(super) struct EncodedCommandResult {
    pub payload: Vec<u8>,
    pub proxy: Option<CompetitionProxyCommandOutcome>,
}

/// 按 Auction-BF `DeterministicID` 的逐字算法派生竞价事实或代理授权 ID。
pub(super) fn deterministic_id(command_id: CommandId, namespace: &str, index: u32) -> Uuid {
    let mut digest = Sha256::new();
    digest.update(namespace.as_bytes());
    digest.update(command_id.as_uuid().as_bytes());
    digest.update(index.to_string().as_bytes());
    let digest = digest.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

/// 严格读取一条旧版成功结果，并映射为只用于无副作用重放的内部计划。
pub(super) fn replay(
    command: &CompetitionCommand,
    payload: &[u8],
) -> Result<CompetitionCommandOutcome, ApplicationError> {
    let (plan, proxy, effective_end_at) = match command {
        CompetitionCommand::PlaceBid(place) => {
            let result: LegacyBidResult =
                serde_json::from_slice(payload).map_err(|_| legacy_payload_error())?;
            if !result.validate_place_bid(place) {
                return Err(legacy_payload_error());
            }
            let effective_end_at = result.effective_end.clone();
            (result.into_plan(0)?, None, effective_end_at)
        }
        CompetitionCommand::CreateProxy(create) => {
            let result: LegacyProxyResult =
                serde_json::from_slice(payload).map_err(|_| legacy_payload_error())?;
            if !result.validate_create_proxy(create) {
                return Err(legacy_payload_error());
            }
            let plan = result.clone().into_plan(command)?;
            let proxy = Some(result.command_outcome(&plan)?);
            let effective_end_at = result.response_effective_end_at()?;
            (plan, proxy, effective_end_at)
        }
        CompetitionCommand::LowerProxy(lower) => {
            let result: LegacyProxyResult =
                serde_json::from_slice(payload).map_err(|_| legacy_payload_error())?;
            if !result.validate_lower_proxy(lower) {
                return Err(legacy_payload_error());
            }
            let plan = result.clone().into_plan(command)?;
            let proxy = Some(result.command_outcome(&plan)?);
            let effective_end_at = result.response_effective_end_at()?;
            (plan, proxy, effective_end_at)
        }
    };
    plan.validate().map_err(|_| legacy_payload_error())?;
    Ok(CompetitionCommandOutcome { plan, proxy, effective_end_at })
}

/// 把新计算出的计划编码为旧实例能够严格重放的成功结果。
pub(super) fn encode(
    command: &CompetitionCommand,
    plan: &CompetitionPlan,
    previous_transitions: &[CompetitionProxyTransitionRecord],
    quota_limit: u32,
    created_at: DateTime<Utc>,
) -> Result<EncodedCommandResult, ApplicationError> {
    plan.validate().map_err(|_| legacy_payload_error())?;
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

/// 为历史新版 `accepted + CompetitionPlan` 行在同一只读事务内补回旧代理响应投影。
pub(super) fn replay_accepted(
    command: &CompetitionCommand,
    plan: CompetitionPlan,
    persisted_transitions: &[CompetitionProxyTransitionRecord],
    quota_limit: u32,
) -> Result<CompetitionCommandOutcome, ApplicationError> {
    plan.validate().map_err(|_| legacy_payload_error())?;
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
    let state_version = usize::try_from(state.state_version).map_err(|_| legacy_payload_error())?;
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

fn legacy_payload_error() -> ApplicationError {
    CompetitionTransactionError::Failed("旧版竞价命令账本结果损坏".to_owned()).into()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum LegacyBidMechanism {
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
struct LegacyAcceptedBidTransition {
    bid_id: [u8; 16],
    origin_command_id: [u8; 16],
    fact_index: i64,
    lot_id: [u8; 16],
    amount: i64,
    leader_id: [u8; 16],
    effective_end: CompetitionWireTimestamp,
    accepted_state_version: i64,
    mechanism: LegacyBidMechanism,
}

impl LegacyAcceptedBidTransition {
    fn from_domain(value: &AcceptedBid) -> Result<Self, ApplicationError> {
        value.validate().map_err(|_| legacy_payload_error())?;
        Ok(Self {
            bid_id: *value.id.as_uuid().as_bytes(),
            origin_command_id: *value.command_id.as_uuid().as_bytes(),
            fact_index: i64::from(value.fact_index),
            lot_id: *value.lot_id.as_uuid().as_bytes(),
            amount: value.amount.value(),
            leader_id: *value.bidder_id.as_uuid().as_bytes(),
            effective_end: CompetitionWireTimestamp::from_utc(value.effective_end_at),
            accepted_state_version: i64::try_from(value.state_version)
                .map_err(|_| legacy_payload_error())?,
            mechanism: LegacyBidMechanism::from_domain(value.mechanism),
        })
    }

    fn validate(&self, lot_id: Uuid, command_id: Uuid) -> bool {
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

    fn into_domain(self) -> Result<AcceptedBid, ApplicationError> {
        Ok(AcceptedBid {
            id: BidId::from_uuid(Uuid::from_bytes(self.bid_id)),
            command_id: CommandId::from_uuid(Uuid::from_bytes(self.origin_command_id)),
            lot_id: LotId::from_uuid(Uuid::from_bytes(self.lot_id)),
            bidder_id: UserId::from_uuid(Uuid::from_bytes(self.leader_id)),
            fact_index: u32::try_from(self.fact_index).map_err(|_| legacy_payload_error())?,
            amount: BidAmount::from_persisted_positive(self.amount)
                .map_err(|_| legacy_payload_error())?,
            effective_end_at: self.effective_end.instant(),
            state_version: u64::try_from(self.accepted_state_version)
                .map_err(|_| legacy_payload_error())?,
            mechanism: self.mechanism.into_domain(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyBidResult {
    lot_id: [u8; 16],
    amount: i64,
    leader_id: [u8; 16],
    effective_end: CompetitionWireTimestamp,
    state_version: i64,
    facts: Vec<LegacyAcceptedBidTransition>,
}

impl LegacyBidResult {
    fn from_plan(plan: &CompetitionPlan) -> Result<Self, ApplicationError> {
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
            state_version: i64::try_from(plan.final_lot.state_version)
                .map_err(|_| legacy_payload_error())?,
            facts,
        })
    }

    fn validate(&self, lot_id: Uuid, command_id: Uuid) -> bool {
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

    fn validate_place_bid(&self, command: &super::PlaceBidCommand) -> bool {
        if !self.validate(command.lot_id.as_uuid(), command.command_id.as_uuid()) {
            return false;
        }
        let first = &self.facts[0];
        Uuid::from_bytes(first.bid_id)
            == deterministic_id(command.command_id, "auction-bid-fact", 1)
            && first.amount == command.amount.value()
            && Uuid::from_bytes(first.leader_id) == command.bidder_id.as_uuid()
            && first.mechanism == LegacyBidMechanism::Ordinary
    }

    fn into_plan(self, last_proxy_sequence: u64) -> Result<CompetitionPlan, ApplicationError> {
        // 旧 BidResult 不携带 lot 原始起拍价；无副作用重放使用首条已接受事实作为安全下界，
        // 不能把可能更高的最终价反向冒充起拍价。
        let replay_start =
            self.facts.first().map(|fact| fact.amount).ok_or_else(legacy_payload_error)?;
        let start_price =
            BidAmount::from_persisted_positive(replay_start).map_err(|_| legacy_payload_error())?;
        let amount =
            BidAmount::from_persisted_positive(self.amount).map_err(|_| legacy_payload_error())?;
        let leader_id = UserId::from_uuid(Uuid::from_bytes(self.leader_id));
        let leader_proxy_last =
            self.facts.last().is_some_and(|fact| fact.mechanism == LegacyBidMechanism::Proxy);
        let public_bids = self
            .facts
            .into_iter()
            .map(LegacyAcceptedBidTransition::into_domain)
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
                state_version: u64::try_from(self.state_version)
                    .map_err(|_| legacy_payload_error())?,
                last_proxy_sequence,
                slow_generation: 0,
                slow_authorization_id: None,
                slow_due_at: None,
            },
            soft_close_applied: false,
        };
        plan.validate().map_err(|_| legacy_payload_error())?;
        Ok(plan)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum LegacyProxyMode {
    Fast,
    Slow,
}

impl LegacyProxyMode {
    const fn from_domain(value: ProxyMode) -> Self {
        match value {
            ProxyMode::Fast => Self::Fast,
            ProxyMode::Slow => Self::Slow,
        }
    }

    const fn into_domain(self) -> ProxyMode {
        match self {
            Self::Fast => ProxyMode::Fast,
            Self::Slow => ProxyMode::Slow,
        }
    }

    fn from_storage(value: &str) -> Option<Self> {
        match value {
            "fast" => Some(Self::Fast),
            "slow" => Some(Self::Slow),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum LegacyProxyStatus {
    Active,
    Exhausted,
    Frozen,
    Closed,
}

impl LegacyProxyStatus {
    const fn from_domain(value: ProxyStatus) -> Self {
        match value {
            ProxyStatus::Active | ProxyStatus::FinalFast => Self::Active,
            ProxyStatus::Exhausted => Self::Exhausted,
            ProxyStatus::Frozen => Self::Frozen,
            ProxyStatus::Closed => Self::Closed,
        }
    }

    const fn into_domain(self) -> ProxyStatus {
        match self {
            Self::Active => ProxyStatus::Active,
            Self::Exhausted => ProxyStatus::Exhausted,
            Self::Frozen => ProxyStatus::Frozen,
            Self::Closed => ProxyStatus::Closed,
        }
    }

    fn from_storage(value: &str) -> Option<Self> {
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
enum LegacyProxyTerminalReason {
    StrictlyDefeated,
    RankedBelow,
    EligibilityLost,
    CompetitionClosed,
}

impl LegacyProxyTerminalReason {
    const fn from_domain(value: ProxyTerminalReason) -> Self {
        match value {
            ProxyTerminalReason::StrictlyDefeated => Self::StrictlyDefeated,
            ProxyTerminalReason::RankedBelow => Self::RankedBelow,
            ProxyTerminalReason::EligibilityLost => Self::EligibilityLost,
            ProxyTerminalReason::CompetitionClosed => Self::CompetitionClosed,
        }
    }

    const fn into_domain(self) -> ProxyTerminalReason {
        match self {
            Self::StrictlyDefeated => ProxyTerminalReason::StrictlyDefeated,
            Self::RankedBelow => ProxyTerminalReason::RankedBelow,
            Self::EligibilityLost => ProxyTerminalReason::EligibilityLost,
            Self::CompetitionClosed => ProxyTerminalReason::CompetitionClosed,
        }
    }

    fn from_storage(value: &str) -> Option<Self> {
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
enum LegacyProxyTransitionKind {
    Created,
    Lowered,
    FinalFast,
    Exhausted,
    Frozen,
    Closed,
}

impl LegacyProxyTransitionKind {
    const fn from_domain(value: ProxyTransitionKind) -> Self {
        match value {
            ProxyTransitionKind::Created => Self::Created,
            ProxyTransitionKind::Lowered => Self::Lowered,
            ProxyTransitionKind::FinalFast => Self::FinalFast,
            ProxyTransitionKind::Exhausted => Self::Exhausted,
            ProxyTransitionKind::Frozen => Self::Frozen,
            ProxyTransitionKind::Closed => Self::Closed,
        }
    }

    const fn into_domain(self) -> ProxyTransitionKind {
        match self {
            Self::Created => ProxyTransitionKind::Created,
            Self::Lowered => ProxyTransitionKind::Lowered,
            Self::FinalFast => ProxyTransitionKind::FinalFast,
            Self::Exhausted => ProxyTransitionKind::Exhausted,
            Self::Frozen => ProxyTransitionKind::Frozen,
            Self::Closed => ProxyTransitionKind::Closed,
        }
    }

    fn from_storage(value: &str) -> Option<Self> {
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyProxyTransitionRecord {
    transition_number: i64,
    kind: LegacyProxyTransitionKind,
    old_maximum_amount: Option<i64>,
    new_maximum_amount: i64,
    old_mode: Option<LegacyProxyMode>,
    new_mode: LegacyProxyMode,
    old_status: Option<LegacyProxyStatus>,
    new_status: LegacyProxyStatus,
    reason: Option<LegacyProxyTerminalReason>,
    resulting_state_version: i64,
    created_at: CompetitionWireTimestamp,
}

impl LegacyProxyTransitionRecord {
    fn from_storage(value: &CompetitionProxyTransitionRecord) -> Result<Self, ApplicationError> {
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

    fn from_domain(value: &ProxyTransition, created_at: DateTime<Utc>) -> Self {
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

    fn to_outcome(&self) -> Result<CompetitionProxyTransitionOutcome, ApplicationError> {
        Ok(CompetitionProxyTransitionOutcome {
            transition_number: u64::try_from(self.transition_number)
                .map_err(|_| legacy_payload_error())?,
            kind: self.kind.into_domain(),
            old_maximum_amount: self
                .old_maximum_amount
                .map(BidAmount::new)
                .transpose()
                .map_err(|_| legacy_payload_error())?,
            new_maximum_amount: BidAmount::new(self.new_maximum_amount)
                .map_err(|_| legacy_payload_error())?,
            old_mode: self.old_mode.map(LegacyProxyMode::into_domain),
            new_mode: self.new_mode.into_domain(),
            old_status: self.old_status.map(LegacyProxyStatus::into_domain),
            new_status: self.new_status.into_domain(),
            reason: self.reason.map(LegacyProxyTerminalReason::into_domain),
            resulting_state_version: u64::try_from(self.resulting_state_version)
                .map_err(|_| legacy_payload_error())?,
            created_at: self.created_at.clone(),
        })
    }

    fn validate(&self) -> bool {
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

    fn into_domain(self, command_id: CommandId) -> Result<ProxyTransition, ApplicationError> {
        let transition = ProxyTransition {
            authorization_id: ProxyAuthorizationId::from_uuid(Uuid::nil()),
            command_id,
            kind: self.kind.into_domain(),
            old_maximum: self
                .old_maximum_amount
                .map(BidAmount::new)
                .transpose()
                .map_err(|_| legacy_payload_error())?,
            new_maximum: BidAmount::new(self.new_maximum_amount)
                .map_err(|_| legacy_payload_error())?,
            old_mode: self.old_mode.map(LegacyProxyMode::into_domain),
            new_mode: self.new_mode.into_domain(),
            old_status: self.old_status.map(LegacyProxyStatus::into_domain),
            new_status: self.new_status.into_domain(),
            state_version: u64::try_from(self.resulting_state_version)
                .map_err(|_| legacy_payload_error())?,
            reason: self.reason.map(LegacyProxyTerminalReason::into_domain),
        };
        Ok(transition)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyProxyResult {
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
    transitions: Vec<LegacyProxyTransitionRecord>,
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

fn serialize_legacy_proxy_facts<S>(
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

fn deserialize_legacy_proxy_facts<'de, D>(
    deserializer: D,
) -> Result<Vec<LegacyAcceptedBidTransition>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<Vec<LegacyAcceptedBidTransition>>::deserialize(deserializer)
        .map(Option::unwrap_or_default)
}

impl LegacyProxyResult {
    fn response_effective_end_at(&self) -> Result<CompetitionWireTimestamp, ApplicationError> {
        self.bid
            .as_ref()
            .map(|bid| bid.effective_end.clone())
            .or_else(|| self.transitions.last().map(|transition| transition.created_at.clone()))
            .ok_or_else(legacy_payload_error)
    }

    fn from_plan(
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
            authorization_sequence: i64::try_from(state.authorization_sequence)
                .map_err(|_| legacy_payload_error())?,
            usage_number: i64::try_from(state.usage_number).map_err(|_| legacy_payload_error())?,
            quota_used: i64::try_from(state.usage_number).map_err(|_| legacy_payload_error())?,
            quota_limit: i64::from(quota_limit),
            mode: LegacyProxyMode::from_domain(state.mode),
            maximum_amount: state.maximum_amount.value(),
            status: LegacyProxyStatus::from_domain(state.status),
            terminal_reason,
            state_version: i64::try_from(state.state_version)
                .map_err(|_| legacy_payload_error())?,
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

    fn validate_create_proxy(&self, command: &super::CreateProxyCommand) -> bool {
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

    fn validate_lower_proxy(&self, command: &super::LowerProxyCommand) -> bool {
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
    fn command_outcome(
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
        authorization.validate().map_err(|_| legacy_payload_error())?;

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
            quota_used: u64::try_from(self.quota_used).map_err(|_| legacy_payload_error())?,
            quota_limit: u32::try_from(self.quota_limit).map_err(|_| legacy_payload_error())?,
            transitions: self
                .transitions
                .iter()
                .map(LegacyProxyTransitionRecord::to_outcome)
                .collect::<Result<Vec<_>, _>>()?,
            terminal_reason: self.terminal_reason.map(LegacyProxyTerminalReason::into_domain),
        })
    }

    fn into_plan(self, command: &CompetitionCommand) -> Result<CompetitionPlan, ApplicationError> {
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
            authorization_sequence: u64::try_from(self.authorization_sequence)
                .map_err(|_| legacy_payload_error())?,
            usage_number: u64::try_from(self.usage_number).map_err(|_| legacy_payload_error())?,
            maximum_amount: BidAmount::new(self.maximum_amount)
                .map_err(|_| legacy_payload_error())?,
            mode: self.mode.into_domain(),
            status,
            state_version: u64::try_from(self.state_version).map_err(|_| legacy_payload_error())?,
        };
        state.validate().map_err(|_| legacy_payload_error())?;

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
            let mut transition = record.clone().into_domain(command_id)?;
            transition.authorization_id = authorization_id;
            proxy_transitions.push(transition);
        }

        let (public_bids, final_lot) = match self.bid {
            Some(bid) => {
                let plan = bid.into_plan(
                    u64::try_from(self.authorization_sequence)
                        .map_err(|_| legacy_payload_error())?,
                )?;
                (plan.public_bids, plan.final_lot)
            }
            None => {
                let maximum =
                    BidAmount::new(self.maximum_amount).map_err(|_| legacy_payload_error())?;
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
                        last_proxy_sequence: u64::try_from(self.authorization_sequence)
                            .map_err(|_| legacy_payload_error())?,
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
        plan.validate().map_err(|_| legacy_payload_error())?;
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
