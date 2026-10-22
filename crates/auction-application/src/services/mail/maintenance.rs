//! MoogleMail 三页定期维护：Sent 观察留证，以及仅针对精确匹配投递的草稿 discard。
//!
//! Sent 只有唯一身份与完整资产矩阵可证明时才恢复，并把履约重新排队等待收取证据；
//! 未找到 Sent 不证明未发送，未知 Send 永不自动重试。通知必须保留原入站关联并通过有界时间窗；
//! compose 草稿只在收件人、业务 reference、明确 COD 和 Credits/附件数量全部匹配时丢弃。

use std::sync::Arc;

use serde_json::json;
use uuid::Uuid;

use crate::{
    ApplicationError, Clock, MoogleMailboxComposeState, MoogleMailboxDiscard,
    MoogleMailboxEvidenceStore, MoogleMailboxMaintenance, MoogleMailboxSentEntry,
    MoogleMailboxSentState, MoogleMailboxUnprovenSend, RepositoryError,
};

/// 通知类单轮最多核对的投递数；其余留到下一轮，避免占满维护周期。
const NOTICE_RECOVERY_PER_SWEEP: usize = 20;
/// 单条投递允许的维护重排队次数；达到上限后保留人工核对，防止无限重试。
const MAX_SWEEP_RETRIES: i64 = 2;

/// 单轮维护结果，只包含安全统计，不含邮件正文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailboxMaintenanceReport {
    pub compose_clean: bool,
    pub matched_delivery: Option<Uuid>,
    pub discarded: bool,
    pub compose_matches: usize,
    pub sent_observed: usize,
    pub sent_recovered: usize,
    pub sent_candidates: usize,
    pub sent_matched: usize,
    pub sent_state_failed: usize,
    /// 最近一次详情读取失败的安全分类，便于定位协议漂移。
    pub sent_state_error: Option<String>,
    /// 本轮退回收发队列重试的投递数。
    pub sweep_requeued: usize,
}

/// 维护服务；由 Worker 在固定周期调用，不存在自动重试队列。
pub struct MailboxMaintenanceService {
    maintenance: Arc<dyn MoogleMailboxMaintenance>,
    store: Arc<dyn MoogleMailboxEvidenceStore>,
    clock: Arc<dyn Clock>,
}

impl MailboxMaintenanceService {
    pub fn new(
        maintenance: Arc<dyn MoogleMailboxMaintenance>,
        store: Arc<dyn MoogleMailboxEvidenceStore>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self { maintenance, store, clock }
    }

    /// 读取 Sent 与 compose 各一次；任何失败都只让本轮维护失败，不影响收件轮询。
    pub async fn sweep(
        &self,
        world_id: &str,
    ) -> Result<MailboxMaintenanceReport, ApplicationError> {
        if world_id != "main" {
            return Err(repository_error("维护流程只支持主世界"));
        }
        let candidates = self.store.unproven_sends(world_id, 200).await?;
        let sent = self.recover_sent(world_id, &candidates).await?;
        let sent_observed = sent.0;
        let sent_recovered = sent.1;
        let compose = self.maintenance.compose(world_id).await?;
        if compose.clean {
            return Ok(MailboxMaintenanceReport {
                compose_clean: true,
                matched_delivery: None,
                discarded: false,
                compose_matches: 0,
                sent_observed,
                sent_recovered,
                sent_candidates: sent.2,
                sent_matched: sent.3,
                sent_state_failed: sent.4,
                sent_state_error: sent.5.clone(),
                sweep_requeued: sent.6,
            });
        }
        let matched = candidates
            .iter()
            .filter(|candidate| compose_matches(candidate, &compose))
            .collect::<Vec<_>>();
        if matched.len() != 1 {
            return Ok(MailboxMaintenanceReport {
                compose_clean: false,
                matched_delivery: None,
                discarded: false,
                compose_matches: matched.len(),
                sent_observed,
                sent_recovered,
                sent_candidates: sent.2,
                sent_matched: sent.3,
                sent_state_failed: sent.4,
                sent_state_error: sent.5.clone(),
                sweep_requeued: sent.6,
            });
        }
        let delivery_id = matched[0].delivery_id;
        let outcome = self.maintenance.discard_compose(world_id).await?;
        let discarded = outcome == MoogleMailboxDiscard::Discarded;
        if discarded {
            self.store
                .record_compose_discard(
                    delivery_id,
                    json!({
                        "observed_at": self.clock.now(),
                        "outcome": "discarded",
                        "matched_reference": matched[0].reference,
                        "recipient_matches": true,
                        "cod_matches": true,
                    }),
                )
                .await?;
        }
        Ok(MailboxMaintenanceReport {
            compose_clean: outcome == MoogleMailboxDiscard::AlreadyClean || discarded,
            matched_delivery: Some(delivery_id),
            discarded,
            compose_matches: 1,
            sent_observed,
            sent_recovered,
            sent_candidates: sent.2,
            sent_matched: sent.3,
            sent_state_failed: sent.4,
            sent_state_error: sent.5.clone(),
            sweep_requeued: sent.6,
        })
    }

    async fn recover_sent(
        &self,
        world_id: &str,
        candidates: &[MoogleMailboxUnprovenSend],
    ) -> Result<(usize, usize, usize, usize, usize, Option<String>, usize), ApplicationError> {
        if candidates.is_empty() {
            // 没有未证明投递时仍完整读取一次 Sent，保持三页周期读取。
            self.maintenance.sent_entries(world_id).await?;
            return Ok((0, 0, 0, 0, 0, None, 0));
        }
        let entries = self.maintenance.sent_entries(world_id).await?;
        // 通知类每轮最多核对固定条数：逐封详情读取各自持有排他会话，全量会拖长维护周期。
        let mut notice_budget = NOTICE_RECOVERY_PER_SWEEP;
        let mut observed = 0;
        let mut recovered = 0;
        let mut matched_count = 0;
        let mut failed = 0;
        let mut requeued = 0;
        let mut state_error = None;
        for candidate in candidates {
            let step = match (candidate.origin_kind.as_str(), candidate.operation.as_str()) {
                ("settlement", "send") => {
                    self.recover_settlement(world_id, candidate, &entries).await?
                }
                ("mail_intake", "send") => {
                    if notice_budget == 0 {
                        continue;
                    }
                    notice_budget -= 1;
                    self.recover_notice(world_id, candidate, &entries).await?
                }
                ("mail_intake", "take") => {
                    if notice_budget == 0 {
                        continue;
                    }
                    notice_budget -= 1;
                    self.recover_take(world_id, candidate).await?
                }
                _ => continue,
            };
            if step.matched {
                matched_count += 1;
            }
            if step.observed {
                observed += 1;
            }
            if step.recovered {
                recovered += 1;
            }
            if step.failed {
                failed += 1;
            }
            if step.requeued {
                requeued += 1;
            }
            if step.error.is_some() {
                state_error = step.error;
            }
        }
        Ok((observed, recovered, candidates.len(), matched_count, failed, state_error, requeued))
    }

    /// 结算类：标题携带义务 reference，详情必须仍带对应装备或 carrier。
    async fn recover_settlement(
        &self,
        world_id: &str,
        candidate: &MoogleMailboxUnprovenSend,
        entries: &[MoogleMailboxSentEntry],
    ) -> Result<RecoveryStep, ApplicationError> {
        if candidate.reference.is_empty() {
            return Ok(RecoveryStep::default());
        }
        let hits =
            entries.iter().filter(|entry| sent_matches(candidate, entry)).collect::<Vec<_>>();
        if hits.len() != 1 {
            return Ok(RecoveryStep::default());
        }
        let mut step = RecoveryStep { matched: true, ..RecoveryStep::default() };
        let Some(state) = self.sent_state(world_id, hits[0], &mut step).await? else {
            return Ok(step);
        };
        // 列表标题被截断，详情完整标题必须再次包含完整 reference。
        if !state.subject.contains(&candidate.reference) {
            return Ok(step);
        }
        self.store
            .record_sent_observation(
                candidate.delivery_id,
                json!({
                    "observed_at": self.clock.now(),
                    "mid": state.external_mail_id,
                    "collected": state.collected,
                    "cod_pending": state.cod_pending,
                    "remaining_credits": state.remaining_credits,
                    "remaining_equipment_count": state.remaining_equipment_ids.len(),
                }),
            )
            .await?;
        step.observed = true;
        if !recovered_state_matches(candidate, &state) {
            return Ok(step);
        }
        // 恢复证明只覆盖身份与金额/资产矩阵；实时发送仍必须走 exact payload 证明。
        step.recovered = self
            .store
            .bind_recovered_send(
                candidate.delivery_id,
                state.external_mail_id.clone(),
                json!({
                    "observed_at": self.clock.now(),
                    "operation": "send",
                    "proof": "sent_recovered_reference_v1",
                    "external_reference_proven": true,
                    "sent_recovered": true,
                    "recovered_mid": state.external_mail_id,
                    "collected": state.collected,
                    "cod_pending": state.cod_pending,
                    "recipient": candidate.recipient,
                    "template": candidate.template,
                }),
            )
            .await?;
        Ok(step)
    }

    /// 通知需有原入站关联、精确正文及发送围栏后的有界时间证明；重复通用模板失败关闭。
    async fn recover_notice(
        &self,
        world_id: &str,
        candidate: &MoogleMailboxUnprovenSend,
        entries: &[MoogleMailboxSentEntry],
    ) -> Result<RecoveryStep, ApplicationError> {
        if candidate.credits != 0 || candidate.cod != 0 || !candidate.attachment_ids.is_empty() {
            return Ok(RecoveryStep::default());
        }
        let expectation = match self.maintenance.notice_expectation(candidate).await {
            Ok(Some(expectation)) => expectation,
            Ok(None) => return Ok(RecoveryStep::default()),
            Err(error) => {
                return Ok(RecoveryStep {
                    failed: true,
                    error: Some(error.to_string()),
                    ..RecoveryStep::default()
                });
            }
        };
        let hits = entries
            .iter()
            .filter(|entry| {
                candidate.recipient.eq_ignore_ascii_case(&entry.recipient)
                    && notice_subject_matches(&expectation.subject, &entry.subject)
                    // 页面时间仅精确到分钟；不向下取整 fence，不把更早同分钟邮件当作本次发送。
                    // 模板原入站关联由 expectation owner 保证，时间窗本身不是发送身份。
                    && entry.sent_at >= candidate.sending_at
                    && entry.sent_at <= candidate.outcome_recorded_at
                    && entry.sent_at <= self.clock.now()
            })
            .collect::<Vec<_>>();
        if hits.len() != 1 {
            return Ok(RecoveryStep::default());
        }
        let mut step = RecoveryStep { matched: true, ..RecoveryStep::default() };
        let Some(state) = self.sent_state(world_id, hits[0], &mut step).await? else {
            return Ok(step);
        };
        if state.subject != expectation.subject
            || state.body_digest != expectation.body_digest
            || state.remaining_credits != 0
            || state.cod_pending != 0
            || !state.remaining_equipment_ids.is_empty()
        {
            return Ok(step);
        }
        self.store
            .record_sent_observation(
                candidate.delivery_id,
                json!({
                    "observed_at": self.clock.now(),
                    "mid": state.external_mail_id,
                    "collected": state.collected,
                    "cod_pending": state.cod_pending,
                    "remaining_credits": state.remaining_credits,
                    "remaining_equipment_count": state.remaining_equipment_ids.len(),
                }),
            )
            .await?;
        step.observed = true;
        step.recovered = self
            .store
            .bind_recovered_notice(
                candidate.delivery_id,
                state.external_mail_id.clone(),
                json!({
                    "observed_at": self.clock.now(),
                    "operation": "send",
                    "proof": "sent_recovered_notice_v1",
                    "external_reference_proven": true,
                    "sent_recovered": true,
                    "recovered_mid": state.external_mail_id,
                    "recipient": candidate.recipient,
                    "template": candidate.template,
                }),
            )
            .await?;
        Ok(step)
    }

    /// take 动作：入站邮件已消失或目标附件已不在，视为已领取；仍有附件则重试。
    async fn recover_take(
        &self,
        world_id: &str,
        candidate: &MoogleMailboxUnprovenSend,
    ) -> Result<RecoveryStep, ApplicationError> {
        let assets = match self.maintenance.inbox_assets(world_id, &candidate.inbound_mail_id).await
        {
            Ok(value) => value,
            Err(error) => {
                return Ok(RecoveryStep {
                    failed: true,
                    error: Some(error.to_string()),
                    ..RecoveryStep::default()
                });
            }
        };
        let target = candidate.attachment_ids.iter().collect::<std::collections::HashSet<_>>();
        match assets {
            None => self.bind_take(candidate, "inbox_mail_absent").await,
            Some(present) => {
                let remaining =
                    present.equipment_ids.iter().filter(|id| target.contains(id)).count();
                if remaining == 0 && present.credits == 0 && present.cod == 0 {
                    self.bind_take(candidate, "attachments_absent").await
                } else {
                    self.requeue(world_id, candidate, "take_assets_still_present").await
                }
            }
        }
    }

    async fn bind_take(
        &self,
        candidate: &MoogleMailboxUnprovenSend,
        reason: &str,
    ) -> Result<RecoveryStep, ApplicationError> {
        let recovered = self
            .store
            .bind_recovered_action(
                candidate.delivery_id,
                candidate.inbound_mail_id.clone(),
                json!({
                    "observed_at": self.clock.now(),
                    "operation": "take",
                    "proof": "action_recovered_inbox_v1",
                    "external_reference_proven": true,
                    "recovered_mid": candidate.inbound_mail_id,
                    "reason": reason,
                    "attachment_count": candidate.attachment_ids.len(),
                }),
            )
            .await?;
        Ok(RecoveryStep { recovered, ..RecoveryStep::default() })
    }

    async fn requeue(
        &self,
        world_id: &str,
        candidate: &MoogleMailboxUnprovenSend,
        reason: &str,
    ) -> Result<RecoveryStep, ApplicationError> {
        if candidate.sweep_retries >= MAX_SWEEP_RETRIES {
            return Ok(RecoveryStep::default());
        }
        let requeued = self
            .store
            .requeue_unproven(candidate.delivery_id, candidate.sweep_retries + 1, reason)
            .await?;
        let _ = world_id;
        Ok(RecoveryStep { requeued, ..RecoveryStep::default() })
    }

    async fn sent_state(
        &self,
        world_id: &str,
        entry: &MoogleMailboxSentEntry,
        step: &mut RecoveryStep,
    ) -> Result<Option<MoogleMailboxSentState>, ApplicationError> {
        match self.maintenance.sent_state(world_id, entry, self.clock.now()).await {
            Ok(state) => Ok(Some(state)),
            Err(error) => {
                step.failed = true;
                step.error = Some(error.to_string());
                Ok(None)
            }
        }
    }
}

/// 单条候选的恢复进度；只用于本轮统计，不进入持久状态。
#[derive(Default)]
struct RecoveryStep {
    matched: bool,
    observed: bool,
    recovered: bool,
    requeued: bool,
    failed: bool,
    error: Option<String>,
}

/// 恢复仅接受尚未领取的完整冻结资产矩阵；领取后原 Credits/COD 已不可从剩余值证明。
fn recovered_state_matches(
    candidate: &MoogleMailboxUnprovenSend,
    state: &MoogleMailboxSentState,
) -> bool {
    if state.collected
        || state.cod_pending != candidate.cod
        || state.remaining_credits != candidate.credits
        || state.remaining_equipment_ids != candidate.attachment_ids
    {
        return false;
    }
    match candidate.template.as_str() {
        "settlement_payment_carrier" => {
            candidate.credits == 1 && candidate.attachment_ids.is_empty()
        }
        "settlement_seller_payout" | "settlement_buyer_refund" => {
            candidate.credits > 0 && candidate.cod == 0 && candidate.attachment_ids.is_empty()
        }
        "settlement_winner_equipment_cod" | "settlement_equipment_handoff" => {
            candidate.credits == 0 && candidate.attachment_ids.len() == 1
        }
        _ => false,
    }
}

/// 通知主题在列表页会按字节截断；多字节截断点会变成 U+FFFD，也可能带省略号。
fn notice_subject_matches(expected: &str, list_subject: &str) -> bool {
    if list_subject.is_empty() {
        return false;
    }
    if expected.starts_with(list_subject) {
        return true;
    }
    let without_replacement = list_subject.trim_end_matches('\u{FFFD}');
    if !without_replacement.is_empty() && expected.starts_with(without_replacement) {
        return true;
    }
    without_replacement
        .strip_suffix("\u{2026}")
        .or_else(|| without_replacement.strip_suffix("..."))
        .is_some_and(|prefix| !prefix.is_empty() && expected.starts_with(prefix))
}

fn sent_matches(candidate: &MoogleMailboxUnprovenSend, entry: &MoogleMailboxSentEntry) -> bool {
    // Sent 列表标题会被服务端截断到固定长度；只比较可证明的 UUID 前缀，完整标题由详情确认。
    candidate.recipient.eq_ignore_ascii_case(&entry.recipient)
        && reference_is_subject_projection(&entry.subject, &candidate.reference)
}

fn reference_is_subject_projection(subject: &str, reference: &str) -> bool {
    let Some(start) = subject.find('[') else { return false };
    let tail = &subject[start + 1..];
    let prefix = tail.split(']').next().unwrap_or(tail).trim();
    prefix.len() >= 6 && reference.starts_with(prefix)
}

fn compose_matches(
    candidate: &MoogleMailboxUnprovenSend,
    compose: &MoogleMailboxComposeState,
) -> bool {
    // 通知类没有业务 reference；空串会匹配任意草稿，必须显式排除。
    candidate.operation == "send"
        && !candidate.reference.is_empty()
        && candidate.recipient.eq_ignore_ascii_case(&compose.recipient)
        && compose.cod == Some(candidate.cod)
        && compose.credits == candidate.credits
        && compose.attachment_count == candidate.attachment_ids.len()
        && (compose.subject.contains(&candidate.reference)
            || compose.body.contains(&candidate.reference))
}

fn repository_error(message: &str) -> ApplicationError {
    RepositoryError::Failed(message.to_owned()).into()
}
