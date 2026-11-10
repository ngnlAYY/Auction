//! `--apply` 前的 Sent 精确匹配计划：列表候选、详情复核、隔离与未决阻塞投影。
//!
//! 本模块只做纯计算，不访问数据库或远端；判定依据来自恢复库的冻结发送尝试或
//! 恢复库 payload 重建的身份，以及 Worker 恢复扫描返回的 Sent 列表与详情。

use std::collections::{BTreeMap, BTreeSet};

use auction_application::operations::{
    WorkerRecoveryInboundEntry, WorkerRecoveryMailRef, WorkerSentLookupCandidate,
};
use auction_application::recovery::{
    EXTERNAL_RECOVERY_AMBIGUOUS_REASON, ExternalRecoveryDeliveryExpectation,
    ExternalRecoveryQuarantineMatch, ExternalRecoverySendEvidence, ExternalRecoveryUnresolvedBlock,
    MAX_EXTERNAL_RECOVERY_BLOCK_MAIL_IDS, SentDetailMatch, sent_detail_match,
    sent_list_projection_may_match,
};
use chrono::{DateTime, TimeDelta, Timelike, Utc};
use serde_json::{Value, json};
use uuid::Uuid;

/// 精确匹配计划；只在 `--apply` 时转换为隔离匹配。
#[derive(Debug, Default)]
pub(super) struct SentMatchPlan {
    pub(super) expectations: Vec<ExternalRecoveryDeliveryExpectation>,
    /// 原始恢复截止；精确匹配只接受不早于该时刻的详情。
    pub(super) cutoff_at: Option<DateTime<Utc>>,
    /// Sent 分钟精度导致的候选下界；只用于筛选候选，不能用于宣称精确匹配。
    pub(super) scan_from: Option<DateTime<Utc>>,
    /// 有列表候选但尚未读取详情的投递 → 候选 MID。
    pub(super) pending_details: Vec<(usize, SentListCandidate)>,
    pub(super) detail_refs: BTreeSet<WorkerRecoveryMailRef>,
    pub(super) exact: Vec<ExactDeliveryMatch>,
    /// 每条投递观察到的 Sent 列表候选 MID；只用于回执与未决阻塞细节。
    pub(super) observed: BTreeMap<usize, BTreeSet<String>>,
    /// 无法唯一归属 Sent 的投递；`--apply` 必须把它们显式阻塞，而不是继续可领取。
    pub(super) ambiguous_deliveries: BTreeSet<usize>,
    /// 有列表候选但详情缺失/不唯一而无法确认的投递；同样不能留在 pending。
    pub(super) unconfirmed_deliveries: BTreeSet<usize>,
    /// 有冻结发送尝试的投递数；其余只能用恢复库 payload 重建身份。
    pub(super) frozen_count: usize,
    pub(super) restored_payload_count: usize,
    pub(super) candidates: usize,
    pub(super) ambiguous: usize,
    pub(super) asset_mismatch: usize,
    pub(super) identity_mismatch: usize,
    /// 有列表候选但第二阶段没有返回详情（能力失败或截断）。
    pub(super) unconfirmed: usize,
}

#[derive(Debug, Clone)]
pub(super) struct SentListCandidate {
    pub(super) world_id: String,
    pub(super) external_mail_id: String,
}

#[derive(Debug, Clone)]
pub(super) struct ExactDeliveryMatch {
    pub(super) delivery_id: Uuid,
    pub(super) fencing_version: i64,
    pub(super) evidence: ExternalRecoverySendEvidence,
    pub(super) world_id: String,
    pub(super) external_mail_id: String,
    pub(super) sent_at: DateTime<Utc>,
    pub(super) recipient: String,
    pub(super) subject: String,
    pub(super) expected_credits: i64,
    pub(super) expected_cod: i64,
    pub(super) expected_equipment_ids: Option<Vec<String>>,
    pub(super) collected: bool,
    pub(super) observed_at: DateTime<Utc>,
}

/// `--apply` 前必须证明 Sent 列表与详情完整，且待核对投递未被截断。
pub(super) fn apply_scan_blockers(
    scan_paused: bool,
    sent_complete: bool,
    scan_errors: &[String],
    detail_errors: &[String],
    detail_unavailable: bool,
    truncated: bool,
) -> Vec<&'static str> {
    let mut blockers = Vec::new();
    if !scan_paused {
        blockers.push("scan_pause_resumed");
    }
    if !sent_complete {
        blockers.push("sent_scan_incomplete");
    }
    if !scan_errors.is_empty() {
        blockers.push("sent_scan_errors");
    }
    if !detail_errors.is_empty() {
        blockers.push("sent_detail_errors");
    }
    if detail_unavailable {
        blockers.push("sent_detail_unavailable");
    }
    if truncated {
        blockers.push("pending_claimed_truncated");
    }
    blockers
}

/// Sent 页面只有分钟精度；恢复截止按所在分钟下界比较，保留整个重叠分钟。
fn cutoff_minute(cutoff_at: DateTime<Utc>) -> DateTime<Utc> {
    cutoff_at
        - TimeDelta::seconds(i64::from(cutoff_at.second()))
        - TimeDelta::nanoseconds(i64::from(cutoff_at.timestamp_subsec_nanos()))
}

impl SentMatchPlan {
    pub(super) fn new(
        expectations: &[ExternalRecoveryDeliveryExpectation],
        cutoff_at: DateTime<Utc>,
    ) -> Self {
        let frozen_count = expectations
            .iter()
            .filter(|expectation| expectation.evidence == ExternalRecoverySendEvidence::Frozen)
            .count();
        Self {
            expectations: expectations.to_vec(),
            // Sent 列表/详情只精确到分钟；分钟下界只扩大候选，精确匹配仍使用原始 cutoff。
            // 与 cutoff 重叠的分钟无法证明实际发送时刻，后续必须进入未决阻塞。
            cutoff_at: Some(cutoff_at),
            scan_from: Some(cutoff_minute(cutoff_at)),
            frozen_count,
            restored_payload_count: expectations.len().saturating_sub(frozen_count),
            ..Self::default()
        }
    }

    /// 先用列表投影筛选详情候选；只有唯一候选才进入第二阶段，多候选即未决。
    pub(super) fn scan_entries(
        &mut self,
        entries: &[auction_application::operations::WorkerRecoverySentEntry],
    ) {
        let Some(cutoff_at) = self.scan_from else { return };
        for (index, expectation) in self.expectations.iter().enumerate() {
            let candidates = entries
                .iter()
                .filter(|entry| {
                    sent_list_projection_may_match(
                        expectation,
                        &entry.world,
                        &entry.recipient,
                        &entry.subject,
                        entry.sent_at,
                        cutoff_at,
                    )
                })
                .collect::<Vec<_>>();
            if candidates.is_empty() {
                continue;
            }
            let mut observed = BTreeSet::new();
            for entry in &candidates {
                observed.insert(entry.external_mail_id.clone());
            }
            self.observed.insert(index, observed);
            match candidates.as_slice() {
                [entry] => {
                    self.candidates += 1;
                    self.pending_details.push((
                        index,
                        SentListCandidate {
                            world_id: entry.world.clone(),
                            external_mail_id: entry.external_mail_id.clone(),
                        },
                    ));
                    self.detail_refs.insert(WorkerRecoveryMailRef {
                        world: entry.world.clone(),
                        external_mail_id: entry.external_mail_id.clone(),
                    });
                }
                _ => {
                    self.ambiguous += 1;
                    self.ambiguous_deliveries.insert(index);
                }
            }
        }
    }

    pub(super) fn apply_details(&mut self, details: &[WorkerSentLookupCandidate]) {
        let Some(cutoff_at) = self.cutoff_at else { return };
        let mut by_mid: BTreeMap<&str, Vec<&WorkerSentLookupCandidate>> = BTreeMap::new();
        for detail in details {
            by_mid.entry(detail.external_mail_id.as_str()).or_default().push(detail);
        }
        let pending = std::mem::take(&mut self.pending_details);
        let mut matches = Vec::new();
        for (delivery_index, candidate) in pending {
            let Some(details) = by_mid.get(candidate.external_mail_id.as_str()) else {
                self.unconfirmed += 1;
                self.unconfirmed_deliveries.insert(delivery_index);
                continue;
            };
            if details.len() != 1 {
                self.ambiguous += 1;
                self.ambiguous_deliveries.insert(delivery_index);
                continue;
            }
            let detail = details[0];
            let Some(sent_at) = detail.sent_at else {
                self.unconfirmed += 1;
                self.unconfirmed_deliveries.insert(delivery_index);
                continue;
            };
            // 详情与列表一样只到分钟；早于原始 cutoff 但在同一分钟的 Sent 无法证明
            // 实际发送时刻。先用分钟下界核对身份/资产：明确不匹配的消息不能阻塞投递；
            // 只有仍可能属于该投递的候选才按时间不确定进入未决阻塞。
            let scan_from = self.scan_from.unwrap_or(cutoff_at);
            if sent_at < scan_from {
                self.identity_mismatch += 1;
                continue;
            }
            let time_ambiguous = sent_at < cutoff_at;
            let match_cutoff = if time_ambiguous { scan_from } else { cutoff_at };
            let expectation = &self.expectations[delivery_index];
            match sent_detail_match(
                expectation,
                &candidate.world_id,
                &detail.recipient,
                &detail.subject,
                sent_at,
                match_cutoff,
                detail.collected,
                detail.remaining_credits,
                detail.cod_pending,
                &detail.remaining_equipment_ids,
            ) {
                SentDetailMatch::Exact if time_ambiguous => {
                    self.ambiguous += 1;
                    self.ambiguous_deliveries.insert(delivery_index);
                }
                SentDetailMatch::Exact => {
                    // 未领取 pending 投递的 fence=0；隔离证据表要求 fence>=1，且不能把
                    // 隔离前 fence 静默改写成 1。此类精确匹配改走未决阻塞（manual_reconcile），
                    // 审计保留原 fence=0，绝不重发。
                    if self.expectations[delivery_index].fencing_version == 0 {
                        self.ambiguous_deliveries.insert(delivery_index);
                    } else {
                        matches.push((delivery_index, candidate, detail.clone(), sent_at));
                    }
                }
                SentDetailMatch::AssetMismatch => self.asset_mismatch += 1,
                SentDetailMatch::IdentityMismatch => self.identity_mismatch += 1,
                // 候选不唯一，或弱证据（payload 重建）无法归因到已领取详情：不能留在
                // pending，必须进入未决阻塞。
                SentDetailMatch::Ambiguous => {
                    self.ambiguous += 1;
                    self.ambiguous_deliveries.insert(delivery_index);
                }
            }
        }
        // 同一 Sent MID 只能证明一个投递；共用证据时两者都不隔离，而是显式阻塞。
        let mut mid_counts: BTreeMap<String, usize> = BTreeMap::new();
        for (_, _, detail, _) in &matches {
            *mid_counts.entry(detail.external_mail_id.clone()).or_default() += 1;
        }
        for (delivery_index, candidate, detail, sent_at) in matches {
            if mid_counts.get(&detail.external_mail_id).copied().unwrap_or(0) > 1 {
                self.ambiguous += 1;
                self.ambiguous_deliveries.insert(delivery_index);
                continue;
            }
            let expectation = &self.expectations[delivery_index];
            self.exact.push(ExactDeliveryMatch {
                delivery_id: expectation.delivery_id,
                fencing_version: expectation.fencing_version,
                evidence: expectation.evidence,
                world_id: candidate.world_id,
                external_mail_id: detail.external_mail_id,
                sent_at,
                recipient: detail.recipient,
                subject: detail.subject,
                expected_credits: expectation.credits,
                expected_cod: expectation.cod,
                expected_equipment_ids: expectation.equipment_ids.clone(),
                collected: detail.collected,
                observed_at: detail.observed_at,
            });
        }
    }

    pub(super) fn quarantine_matches(&self) -> Vec<ExternalRecoveryQuarantineMatch> {
        self.exact
            .iter()
            .map(|exact| ExternalRecoveryQuarantineMatch {
                delivery_id: exact.delivery_id,
                fencing_version: exact.fencing_version,
                world_id: exact.world_id.clone(),
                external_mail_id: exact.external_mail_id.clone(),
                sent_at: exact.sent_at,
                recipient: exact.recipient.clone(),
                subject: exact.subject.clone(),
                expected_credits: exact.expected_credits,
                expected_cod: exact.expected_cod,
                expected_equipment_ids: exact.expected_equipment_ids.clone(),
                collected: exact.collected,
                observed_at: exact.observed_at,
            })
            .collect()
    }

    /// 无法唯一归属 Sent 的投递；没有外部证据可写，只按本地状态阻塞。
    pub(super) fn unresolved_blocks(&self) -> Vec<ExternalRecoveryUnresolvedBlock> {
        self.ambiguous_deliveries
            .iter()
            .chain(self.unconfirmed_deliveries.iter())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .filter_map(|index| {
                let expectation = self.expectations.get(*index)?;
                Some(ExternalRecoveryUnresolvedBlock {
                    delivery_id: expectation.delivery_id,
                    fencing_version: expectation.fencing_version,
                    reason: EXTERNAL_RECOVERY_AMBIGUOUS_REASON.to_owned(),
                    candidate_mail_ids: self
                        .observed
                        .get(index)
                        .map(|mids| {
                            mids.iter()
                                .take(MAX_EXTERNAL_RECOVERY_BLOCK_MAIL_IDS)
                                .cloned()
                                .collect()
                        })
                        .unwrap_or_default(),
                })
            })
            .collect()
    }

    /// 有候选但未进入精确隔离的投递投影；回执必须让操作者看到未决项。
    pub(super) fn unresolved_projections(&self) -> Vec<Value> {
        self.ambiguous_deliveries
            .iter()
            .chain(self.unconfirmed_deliveries.iter())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .filter_map(|index| {
                let expectation = self.expectations.get(*index)?;
                Some(json!({
                    "delivery_id": expectation.delivery_id,
                    "fencing_version": expectation.fencing_version,
                    "evidence": evidence_label(expectation.evidence),
                    "world": expectation.world_id,
                    "recipient": expectation.recipient,
                    "frozen_subject_known": expectation.subject.is_some(),
                    "candidate_mail_ids": self.observed.get(index)
                        .map(|mids| mids.iter().cloned().collect::<Vec<_>>())
                        .unwrap_or_default(),
                    "reason": EXTERNAL_RECOVERY_AMBIGUOUS_REASON,
                }))
            })
            .collect()
    }
}

pub(super) const fn evidence_label(evidence: ExternalRecoverySendEvidence) -> &'static str {
    match evidence {
        ExternalRecoverySendEvidence::Frozen => "frozen_expectation",
        ExternalRecoverySendEvidence::RestoredPayload => "restored_payload",
    }
}

/// Read Mail 摘要按恢复库入站身份核对；**不**用到达时间截断。
/// 读取时间没有持久证据，备份前到达、备份后才处理并读取的邮件也可能丢失本地记录。
pub(super) fn missing_inbound(
    entries: &[WorkerRecoveryInboundEntry],
    known: &BTreeSet<(String, String)>,
) -> Vec<Value> {
    entries
        .iter()
        .filter(|entry| !known.contains(&(entry.world.clone(), entry.external_mail_id.clone())))
        .map(|entry| {
            json!({
                "world": entry.world,
                "external_mail_id": entry.external_mail_id,
                "sender": entry.sender,
                "subject": entry.subject,
                "received_at": entry.received_at,
            })
        })
        .collect()
}
