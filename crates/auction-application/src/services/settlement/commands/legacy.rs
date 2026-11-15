//! 旧版两阶段结算命令账本的只读兼容与权威事实对账。

use std::collections::BTreeMap;

use auction_domain::{SettlementObligationKind, SettlementPlan};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::obligations::build_obligations;
use super::validation::{validate_command_identity, validate_settlement_plan};
use crate::errors::ApplicationError;
use crate::errors::diagnostics::{
    DiagnosticContext, DiagnosticError, DiagnosticLevel, record_error,
};
use crate::ports::core::{CommandLedgerError, RequestHash, StoredCommand};
use crate::ports::settlement::{
    PersistedSettlementObligationCount, SettlementSnapshot, SettlementTransactionError,
};
use crate::services::settlement::SettleAuctionCommand;

const LEGACY_SETTLEMENT_ACTOR: &str = "auction-worker";

/// 已保存结算命令所属的线协议世代。
pub(super) enum StoredSettlementReplay {
    Current,
    Legacy(LegacySettlementResult),
}

/// 已发布的结算完成命令摘要格式：Go `encoding/json` 的字段顺序与表示，字节兼容、不可修改，
/// 由本文件的 `legacy_complete_hash_matches_frozen_vectors` 冻结向量固定。
/// ID 按 Go `[16]byte` 编码，因此 JSON 必须是十六个数字组成的数组；不能使用 UUID 字符串。
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
    let due_at = crate::ports::auction::format_public_time_wire(command.due_at);
    let now = crate::ports::auction::format_public_time_wire(command.now);
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
    let result: LegacySettlementResult =
        serde_json::from_slice(&stored.result_payload).map_err(|error| {
            record_error(
                DiagnosticLevel::Error,
                error.diagnostic_kind(),
                "settlement_ledger",
                "decode_legacy_result",
                DiagnosticContext::default(),
            );
            failed("旧版结算命令结果 JSON 损坏")
        })?;
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
        || result.policy_mode != persisted.policy_mode.as_str()
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
            .map_err(|error| {
                record_error(
                    DiagnosticLevel::Error,
                    error.diagnostic_kind(),
                    "settlement_ledger",
                    "convert_sold_count",
                    DiagnosticContext::default(),
                );
                failed("结算成交数量超出安全范围")
            })?;
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
            if *count <= 0 || SettlementObligationKind::from_name(kind).is_none() {
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
    Ok(RequestHash::from_canonical_bytes(&payload))
}

fn persisted_counts(
    counts: &[PersistedSettlementObligationCount],
) -> Result<BTreeMap<&'static str, u64>, ApplicationError> {
    let mut mapped = BTreeMap::new();
    for entry in counts {
        if entry.count == 0 || mapped.insert(entry.kind.as_str(), entry.count).is_some() {
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
        let count = counts.entry(obligation.kind.as_str()).or_insert(0_u64);
        *count = count.checked_add(1).ok_or_else(|| failed("结算义务数量溢出"))?;
    }
    Ok(counts)
}

fn legacy_counts(counts: &BTreeMap<String, i64>) -> Result<BTreeMap<&str, u64>, ApplicationError> {
    counts
        .iter()
        .map(|(kind, count)| {
            if SettlementObligationKind::from_name(kind).is_none() {
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

fn uuid_bytes(value: Uuid) -> [u8; 16] {
    *value.as_bytes()
}

fn failed(message: &str) -> ApplicationError {
    SettlementTransactionError::Failed(message.to_owned()).into()
}

#[cfg(test)]
mod tests {
    use auction_domain::{AuctionId, CommandId};
    use chrono::{DateTime, Utc};
    use uuid::Uuid;

    use super::legacy_complete_hash;
    use crate::ports::core::RequestHash;
    use crate::services::settlement::SettleAuctionCommand;

    fn uuid(value: &str) -> Uuid {
        Uuid::parse_str(value).expect("测试 UUID 必须合法")
    }

    fn at(value: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(value).expect("测试时间必须合法").with_timezone(&Utc)
    }

    /// 旧账本 `settlement_complete` 摘要只能逐字复现；向量由改用共享摘要入口前的实现生成。
    #[test]
    fn legacy_complete_hash_matches_frozen_vectors() {
        let command = SettleAuctionCommand {
            command_id: CommandId::from_uuid(uuid("76a6a45f-06e2-5460-84cb-f200bbdc6910")),
            workflow_id: uuid("a2cfde74-e3a0-5ddc-a6f7-c53ac0359d15"),
            claim_command_id: CommandId::from_uuid(uuid("2b52d9e6-4af0-560e-b9c7-4c8e64b9b4fe")),
            auction_id: AuctionId::from_uuid(uuid("0f8b2c4e-7a1d-4e3b-9c5f-2d6a8e0b1c3d")),
            due_at: at("2026-11-14T12:34:56.123456789Z"),
            now: at("2026-11-14T12:40:00Z"),
            request_hash: RequestHash::new([1; 32]),
        };
        for (due_at, now, expected) in [
            (
                "2026-11-14T12:34:56.123456789Z",
                "2026-11-14T12:40:00Z",
                "91032714332c075532e594a99a0b2ccf3697605508b02d1af4d58fd74e9b9a07",
            ),
            (
                "2026-11-14T12:34:56.123456789Z",
                "2026-11-14T12:40:00.000000000Z",
                "552a960ab73f38bb16104567f77eab54c8b95e316fd8743c07f11c7cb4daf869",
            ),
        ] {
            let actual: String = legacy_complete_hash(command, due_at, now)
                .expect("旧版完成摘要必须可编码")
                .as_bytes()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            assert_eq!(actual, expected, "{due_at} {now}");
        }
    }
}
