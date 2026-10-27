//! 旧版两阶段结算命令账本的只读兼容与权威事实对账。

use std::collections::BTreeMap;

use auction_domain::{SettlementMode, SettlementObligationKind, SettlementPlan};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    ApplicationError, CommandLedgerError, PersistedSettlementObligationCount, RequestHash,
    SettlementSnapshot, SettlementTransactionError, StoredCommand,
};

use super::SettleAuctionCommand;
use super::obligations::build_obligations;
use super::validation::{validate_command_identity, validate_settlement_plan};

const LEGACY_SETTLEMENT_ACTOR: &str = "auction-worker";

/// 已保存结算命令所属的线协议世代。
pub(super) enum StoredSettlementReplay {
    Current,
    Legacy(LegacySettlementResult),
}

/// 旧版 `SettlementCompleteCommand` 的 Go `encoding/json` 字段顺序与表示。
/// 旧版 ID 是 `[16]byte`，因此 JSON 必须是十六个数字组成的数组；不能使用 UUID 字符串。
#[derive(Serialize)]
struct LegacySettlementCompleteHash<'a> {
    #[serde(rename = "CommandID")]
    command_id: [u8; 16],
    #[serde(rename = "WorkflowID")]
    workflow_id: [u8; 16],
    #[serde(rename = "ClaimCommandID")]
    claim_command_id: [u8; 16],
    #[serde(rename = "AuctionID")]
    auction_id: [u8; 16],
    #[serde(rename = "DueAt")]
    due_at: &'a str,
    #[serde(rename = "Actor")]
    actor: &'static str,
    #[serde(rename = "Now")]
    now: &'a str,
}

/// 旧版 `settlement_complete` 的严格结果对象；未知字段必须失败关闭。
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LegacySettlementResult {
    auction_id: [u8; 16],
    policy_version: i64,
    policy_mode: String,
    pool_enabled: bool,
    opening_pool_balance: i64,
    closing_pool_balance: i64,
    opening_pool_version: i64,
    closing_pool_version: i64,
    tax_total: i64,
    discount_demand: i64,
    compensation_total: i64,
    sold_count: i64,
    unsold_count: i64,
    obligation_count: i64,
    obligation_kind_counts: BTreeMap<String, i64>,
    state_version: i64,
    settled_at: DateTime<Utc>,
}

/// 先识别并校验命令账本，再读取或锁定拍卖；这与旧版冲突优先级一致。
pub(super) fn classify_stored_settlement(
    stored: &StoredCommand,
    command: SettleAuctionCommand,
) -> Result<StoredSettlementReplay, ApplicationError> {
    validate_command_identity(command)?;
    if stored.matches_request("settle_auction", command.auction_id.as_uuid(), command.request_hash)
    {
        if stored.result_code != "accepted" {
            return Err(failed("新版结算命令账本结果分类无效"));
        }
        return Ok(StoredSettlementReplay::Current);
    }

    if stored.command_kind != "settlement_complete"
        || stored.aggregate_id != command.auction_id.as_uuid()
    {
        return Err(CommandLedgerError::Conflict.into());
    }
    // Go RFC3339Nano 会去掉小数尾零；保留早期 Rust AutoSi 摘要兼容，不改写既有账本。
    let due_at = crate::format_public_time_wire(command.due_at);
    let now = crate::format_public_time_wire(command.now);
    if stored.request_hash != legacy_complete_hash(command, &due_at, &now)?
        && stored.request_hash
            != legacy_complete_hash(
                command,
                &command.due_at.to_rfc3339_opts(SecondsFormat::AutoSi, true),
                &command.now.to_rfc3339_opts(SecondsFormat::AutoSi, true),
            )?
    {
        return Err(CommandLedgerError::Conflict.into());
    }
    if stored.result_code != "OK" {
        return Err(failed("旧版结算命令账本结果分类无效"));
    }
    let result: LegacySettlementResult = serde_json::from_slice(&stored.result_payload)
        .map_err(|_| failed("旧版结算命令结果 JSON 损坏"))?;
    if !result.valid_for(command) {
        return Err(failed("旧版结算命令结果语义损坏"));
    }
    Ok(StoredSettlementReplay::Legacy(result))
}

/// 把旧版摘要与已迁移的结算表逐项对账，并从权威 lot 事实重建当前领域计划。
/// 这里只允许重放已经 settled 的结果；绝不能拿旧摘要再次执行结算写入。
pub(super) fn replay_legacy_settlement(
    command: SettleAuctionCommand,
    snapshot: &SettlementSnapshot,
    result: LegacySettlementResult,
) -> Result<SettlementPlan, ApplicationError> {
    let persisted = snapshot
        .persisted_settlement
        .as_ref()
        .ok_or_else(|| failed("旧版结算命令缺少已结算权威事实"))?;
    let persisted_counts = persisted_counts(&persisted.obligation_counts)?;
    let legacy_counts = legacy_counts(&result.obligation_kind_counts)?;

    if persisted.command_id != command.command_id
        || persisted.workflow_complete_command_id != Some(command.command_id)
        || persisted.state_version != snapshot.auction_state_version
        || result.auction_id != uuid_bytes(command.auction_id.as_uuid())
        || result.policy_version != i64::from(persisted.policy_version)
        || result.policy_mode != settlement_mode_name(persisted.policy_mode)
        || result.pool_enabled != persisted.pool_enabled
        || result.opening_pool_balance != persisted.opening_pool.value()
        || result.closing_pool_balance != persisted.closing_pool.value()
        || u64::try_from(result.opening_pool_version).ok() != Some(persisted.opening_pool_version)
        || u64::try_from(result.closing_pool_version).ok() != Some(persisted.closing_pool_version)
        || result.tax_total != persisted.tax_total.value()
        || result.discount_demand != persisted.discount_demand.value()
        || result.compensation_total != persisted.compensation_total.value()
        || u64::try_from(result.sold_count).ok() != Some(persisted.sold_count)
        || u64::try_from(result.unsold_count).ok() != Some(persisted.unsold_count)
        || u64::try_from(result.obligation_count).ok()
            != persisted_counts.values().try_fold(0_u64, |total, count| total.checked_add(*count))
        || legacy_counts != persisted_counts
        || u64::try_from(result.state_version).ok() != Some(persisted.state_version)
        || result.settled_at != persisted.settled_at
    {
        return Err(failed("旧版结算摘要与权威持久化事实不一致"));
    }

    let mut plan = SettlementPlan::calculate(
        &snapshot.policy,
        snapshot.scheduled_open_at,
        snapshot.scheduled_end_at,
        persisted.opening_pool,
        &snapshot.facts,
    )?;
    plan.obligations = build_obligations(command.command_id, snapshot, &plan)?;
    validate_settlement_plan(command.command_id, snapshot, &plan)?;
    let generated_counts = generated_counts(&plan)?;
    let sold_count =
        u64::try_from(snapshot.facts.iter().filter(|fact| fact.leader_id.is_some()).count())
            .map_err(|_| failed("结算成交数量超出安全范围"))?;
    let unsold_count = u64::try_from(snapshot.facts.len())
        .ok()
        .and_then(|total| total.checked_sub(sold_count))
        .ok_or_else(|| failed("结算流拍数量超出安全范围"))?;
    if plan.policy_version != persisted.policy_version
        || plan.policy_mode != persisted.policy_mode
        || plan.compensation_pool_enabled != persisted.pool_enabled
        || plan.opening_pool != persisted.opening_pool
        || plan.closing_pool != persisted.closing_pool
        || plan.tax_total != persisted.tax_total
        || plan.discount_demand != persisted.discount_demand
        || plan.compensation_total != persisted.compensation_total
        || plan.lines != persisted.lines
        || sold_count != persisted.sold_count
        || unsold_count != persisted.unsold_count
        || generated_counts != persisted_counts
    {
        return Err(failed("旧版结算结果无法从权威 lot 与义务事实重建"));
    }
    Ok(plan)
}

impl LegacySettlementResult {
    fn valid_for(&self, command: SettleAuctionCommand) -> bool {
        if self.auction_id != uuid_bytes(command.auction_id.as_uuid())
            || self.policy_version < 0
            || self.opening_pool_balance < 0
            || self.closing_pool_balance < 0
            || self.opening_pool_version <= 0
            || self.closing_pool_version <= 0
            || self.tax_total < 0
            || self.discount_demand < 0
            || self.compensation_total < 0
            || self.sold_count < 0
            || self.unsold_count < 0
            || self.obligation_count <= 0
            || self.obligation_kind_counts.is_empty()
            || self.state_version <= 0
            || self.settled_at.timestamp_nanos_opt().is_none()
            || self.discount_demand < self.compensation_total
        {
            return false;
        }
        match self.policy_mode.as_str() {
            "legacy_zero_v0" if self.policy_version == 0 && !self.pool_enabled => {}
            "hold_curve_v1" if self.policy_version == 1 => {}
            _ => return false,
        }
        let pool_valid = if self.pool_enabled {
            self.opening_pool_balance
                .checked_add(self.tax_total)
                .and_then(|available| available.checked_sub(self.compensation_total))
                == Some(self.closing_pool_balance)
                && self.opening_pool_version.checked_add(1) == Some(self.closing_pool_version)
        } else {
            self.closing_pool_version == self.opening_pool_version
                && self.closing_pool_balance == self.opening_pool_balance
                && self.compensation_total == 0
        };
        if !pool_valid
            || !matches!(
                self.sold_count.checked_add(self.unsold_count),
                Some(total) if total > 0
            )
        {
            return false;
        }
        self.obligation_kind_counts.iter().try_fold(0_i64, |total, (kind, count)| {
            if *count <= 0 || !valid_obligation_kind(kind) {
                None
            } else {
                total.checked_add(*count)
            }
        }) == Some(self.obligation_count)
    }
}

fn legacy_complete_hash(
    command: SettleAuctionCommand,
    due_at: &str,
    now: &str,
) -> Result<RequestHash, ApplicationError> {
    let payload = serde_json::to_vec(&LegacySettlementCompleteHash {
        command_id: uuid_bytes(command.command_id.as_uuid()),
        workflow_id: uuid_bytes(command.workflow_id),
        claim_command_id: uuid_bytes(command.claim_command_id.as_uuid()),
        auction_id: uuid_bytes(command.auction_id.as_uuid()),
        due_at,
        actor: LEGACY_SETTLEMENT_ACTOR,
        now,
    })?;
    let digest = Sha256::digest(payload);
    let mut bytes = [0_u8; 32];
    bytes.copy_from_slice(&digest);
    Ok(RequestHash::new(bytes))
}

fn persisted_counts(
    counts: &[PersistedSettlementObligationCount],
) -> Result<BTreeMap<&'static str, u64>, ApplicationError> {
    let mut mapped = BTreeMap::new();
    for entry in counts {
        if entry.count == 0
            || mapped.insert(obligation_kind_name(entry.kind), entry.count).is_some()
        {
            return Err(failed("权威结算义务数量投影无效"));
        }
    }
    if mapped.is_empty() {
        return Err(failed("权威结算义务数量投影为空"));
    }
    Ok(mapped)
}

fn generated_counts(
    plan: &SettlementPlan,
) -> Result<BTreeMap<&'static str, u64>, ApplicationError> {
    let mut counts = BTreeMap::new();
    for obligation in &plan.obligations {
        let count = counts.entry(obligation_kind_name(obligation.kind)).or_insert(0_u64);
        *count = count.checked_add(1).ok_or_else(|| failed("结算义务数量溢出"))?;
    }
    Ok(counts)
}

fn legacy_counts(counts: &BTreeMap<String, i64>) -> Result<BTreeMap<&str, u64>, ApplicationError> {
    counts
        .iter()
        .map(|(kind, count)| {
            if !valid_obligation_kind(kind) {
                return Err(failed("旧版结算义务种类未知"));
            }
            u64::try_from(*count)
                .ok()
                .filter(|count| *count > 0)
                .map(|count| (kind.as_str(), count))
                .ok_or_else(|| failed("旧版结算义务数量无效"))
        })
        .collect()
}

fn settlement_mode_name(mode: SettlementMode) -> &'static str {
    match mode {
        SettlementMode::LegacyZeroV0 => "legacy_zero_v0",
        SettlementMode::HoldCurveV1 => "hold_curve_v1",
        SettlementMode::HoldCurveV2 => "hold_curve_v2",
    }
}

fn obligation_kind_name(kind: SettlementObligationKind) -> &'static str {
    match kind {
        SettlementObligationKind::WinnerEquipmentCod => "winner_equipment_cod",
        SettlementObligationKind::WinnerPaymentCarrierCod => "winner_payment_carrier_cod",
        SettlementObligationKind::WinnerEquipmentHandoff => "winner_equipment_handoff",
        SettlementObligationKind::SellerPayout => "seller_payout",
        SettlementObligationKind::BuyerRefund => "buyer_refund",
        SettlementObligationKind::UnsoldReturn => "unsold_return",
        SettlementObligationKind::WithdrawnReturn => "withdrawn_return",
    }
}

fn valid_obligation_kind(kind: &str) -> bool {
    matches!(
        kind,
        "winner_equipment_cod"
            | "winner_payment_carrier_cod"
            | "winner_equipment_handoff"
            | "seller_payout"
            | "buyer_refund"
            | "unsold_return"
            | "withdrawn_return"
    )
}

fn uuid_bytes(value: Uuid) -> [u8; 16] {
    *value.as_bytes()
}

fn failed(message: &str) -> ApplicationError {
    SettlementTransactionError::Failed(message.to_owned()).into()
}
