//! MoogleMail 周期观察与恢复：Inbox 由持久 intake journal 处理，Read Mail 仅观察。
//!
//! 新 Send 只凭原尝试冻结的主题、正文摘要、世界、收件人和完整资产唯一恢复；
//! 旧行保留原有保守证明，不能以当前登录 key 重建历史发送。未找到 Sent 不证明未发送，
//! 未知 Send 永不自动重试。compose 草稿仍只在精确匹配后持短时独占锁受控丢弃。

use std::sync::Arc;
use std::time::Instant;

use serde_json::json;
use uuid::Uuid;

use crate::errors::ApplicationError;
use crate::errors::diagnostics::{
    DiagnosticContext, DiagnosticDisposition, DiagnosticError, DiagnosticLevel, diagnostic_world,
    record_error,
};
use crate::ports::core::{Clock, RepositoryError};
use crate::ports::delivery::{
    DeliverySendCredentialBinding, DeliverySendExpectation, DeliveryTakeExpectation,
};
use crate::ports::mail::{
    MoogleMailboxComposeState, MoogleMailboxDiscard, MoogleMailboxEvidenceStore,
    MoogleMailboxInboxAssets, MoogleMailboxMaintenance, MoogleMailboxSentEntry,
    MoogleMailboxSentState, MoogleMailboxSweepCandidate, MoogleMailboxUnprovenSend,
    mail_recipient_is_list_projection, sent_list_minute_start,
};

/// 单轮最多核对的投递数；达到上限即停止，不越过未处理后缀推进持久游标。
const RECOVERY_PER_SWEEP: usize = 20;
/// 相同列表投影过多时保留未知，不读取无界详情或只选择前几个结果冒充唯一性。
const EXACT_MATCH_CANDIDATES: usize = 32;
/// 单条结算投递在完整 Sent 中按同主题前缀最多读取的详情数；超出即保守放弃目击。
const SIGHTING_CANDIDATES: usize = 4;
/// 单条投递允许的维护重排队次数；达到上限后保留人工核对，防止无限重试。
const MAX_SWEEP_RETRIES: i64 = 2;

/// 单轮维护结果，只包含安全统计，不含邮件正文。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MailboxMaintenanceReport {
    pub compose_clean: bool,
    pub compose_error: Option<String>,
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

    /// Read Mail 独立预算观察；Worker 与 Sent/compose 维护及 Inbox intake 分别驱动。
    /// 返回值是本轮实际扫描到的行数：增量轮只含水位之后的新增，每日校验轮为完整数量；
    /// 它只是观察计数，不是已读、收取或业务完成证明。
    pub async fn observe_read_mail(
        &self,
        world_id: &str,
        deadline: Instant,
    ) -> Result<usize, ApplicationError> {
        if !matches!(world_id, "main" | "isekai") {
            return Err(repository_error("维护世界无效"));
        }
        check_deadline(deadline)?;
        let observed = self.maintenance.read_mail_observed(world_id).await?;
        check_deadline(deadline)?;
        Ok(observed)
    }

    /// Worker 以同一总截止点包住完整 future；DB、凭据排队与所有远端操作均计入维护预算。
    /// 普通页面的远端失败分开记录，不清除已获证据；游标与 discard 围栏在取消后仍保留。
    pub async fn sweep(
        &self,
        world_id: &str,
        deadline: Instant,
    ) -> Result<MailboxMaintenanceReport, ApplicationError> {
        self.sweep_with_activity(
            world_id,
            deadline,
            &crate::ports::operations::ExternalWriteActivity::default(),
        )
        .await
    }

    /// 活动从本次 discard 围栏调用前覆盖交接与外写；确认一次性尝试提交后才授权 POST。
    pub async fn sweep_with_activity(
        &self,
        world_id: &str,
        deadline: Instant,
        activity: &crate::ports::operations::ExternalWriteActivity,
    ) -> Result<MailboxMaintenanceReport, ApplicationError> {
        if !matches!(world_id, "main" | "isekai") {
            return Err(repository_error("维护世界无效"));
        }
        check_deadline(deadline)?;
        let mut report = MailboxMaintenanceReport::default();
        let candidates = self.store.unproven_sends(world_id, 200).await?;
        let sent = self.recover_sent(world_id, &candidates, deadline).await?;
        report.sent_observed = sent.0;
        report.sent_recovered = sent.1;
        report.sent_candidates = sent.2;
        report.sent_matched = sent.3;
        report.sent_state_failed = sent.4;
        report.sent_state_error = sent.5;
        report.sweep_requeued = sent.6;
        check_deadline(deadline)?;
        let compose = match self.maintenance.compose(world_id).await {
            Ok(compose) => compose,
            Err(error) => {
                record_error(
                    DiagnosticLevel::Warn,
                    error.diagnostic_kind(),
                    "mail_maintenance",
                    "read_compose",
                    DiagnosticContext {
                        world: diagnostic_world(world_id),
                        disposition: Some(DiagnosticDisposition::Unpersisted),
                        ..Default::default()
                    },
                );
                report.compose_error = Some(error.to_string());
                return Ok(report);
            }
        };
        report.compose_clean = compose.clean;
        if compose.clean {
            return Ok(report);
        }
        let mut matches =
            candidates.iter().filter_map(|candidate| candidate.send.as_ref()).filter(|candidate| {
                candidate.world_id == world_id && compose_matches(candidate, &compose)
            });
        let Some(matched) = matches.next() else { return Ok(report) };
        report.compose_matches = 1 + matches.count();
        if report.compose_matches != 1 {
            return Ok(report);
        }
        let delivery_id = matched.delivery_id;
        report.matched_delivery = Some(delivery_id);
        check_deadline(deadline)?;
        // 围栏调用开始即受保护；关闭先赢时不消费一次性尝试，保留原持久状态。
        // 已登记的数据库往返与后续外写共用排空截止，登记本身不授权 discard。
        if !activity.enter_critical() {
            return Ok(report);
        }
        if !self
            .store
            .begin_compose_discard(
                delivery_id,
                matched.fencing_version,
                matched.send_expectation.as_ref(),
            )
            .await?
        {
            report.compose_error = Some("草稿丢弃已尝试或投递状态已变化，保留人工核对".to_owned());
            return Ok(report);
        }
        // 围栏在排队和 POST 前持久提交；其后任意超时或取消都不能自动重新授权 discard。
        check_deadline(deadline)?;
        let outcome = match self.maintenance.discard_compose(world_id, &compose).await {
            Ok(outcome) => outcome,
            Err(error) => {
                record_error(
                    DiagnosticLevel::Warn,
                    error.diagnostic_kind(),
                    "mail_maintenance",
                    "discard_compose",
                    DiagnosticContext {
                        world: diagnostic_world(world_id),
                        disposition: Some(DiagnosticDisposition::Unpersisted),
                        ..Default::default()
                    },
                );
                report.compose_error = Some(error.to_string());
                return Ok(report);
            }
        };
        report.discarded = outcome == MoogleMailboxDiscard::Discarded;
        if report.discarded {
            self.store
                .record_compose_discard(
                    delivery_id,
                    matched.fencing_version,
                    json!({
                        "observed_at": self.clock.now(),
                        "outcome": "discarded",
                        "matched_reference": matched.reference,
                        "recipient_matches": true,
                        "cod_matches": true,
                    }),
                )
                .await?;
            activity.complete();
        }
        report.compose_clean = outcome == MoogleMailboxDiscard::AlreadyClean || report.discarded;
        Ok(report)
    }

    async fn recover_sent(
        &self,
        world_id: &str,
        candidates: &[MoogleMailboxSweepCandidate],
        deadline: Instant,
    ) -> Result<(usize, usize, usize, usize, usize, Option<String>, usize), ApplicationError> {
        // 只有可重建的 Send 候选才读取 Sent；Take 与纯游标轮转不依赖该列表。
        let needs_sent = candidates
            .iter()
            .any(|slot| slot.send.as_ref().is_some_and(|candidate| candidate.operation == "send"));
        let (entries, mut state_error) = if needs_sent {
            match self.maintenance.sent_entries(world_id).await {
                Ok(entries) => (Some(entries), None),
                Err(error) => {
                    record_error(
                        DiagnosticLevel::Warn,
                        error.diagnostic_kind(),
                        "mail_maintenance",
                        "read_sent_entries",
                        DiagnosticContext {
                            world: diagnostic_world(world_id),
                            disposition: Some(DiagnosticDisposition::Unpersisted),
                            ..Default::default()
                        },
                    );
                    (None, Some(error.to_string()))
                }
            }
        } else {
            (None, None)
        };
        // 按已读取的稳定环形批次逐条开始；候选批次本身不推进持久游标。
        let mut recovery_budget = RECOVERY_PER_SWEEP;
        let mut observed = 0;
        let mut attempted = 0;
        let mut recovered = 0;
        let mut matched_count = 0;
        let mut failed = usize::from(state_error.is_some());
        let mut requeued = 0;
        for slot in candidates {
            if recovery_budget == 0 {
                break;
            }
            check_deadline(deadline)?;
            if entries.is_none()
                && slot.send.as_ref().is_some_and(|candidate| candidate.operation == "send")
            {
                // 未获得完整 Sent 列表就没有开始此候选，保留它及其后缀的持久轮转位置。
                break;
            }
            if !self.store.record_sweep_attempt(world_id, slot.delivery_id).await? {
                continue;
            }
            attempted += 1;
            // 不可重建的旧行也获得一次轮转机会，但不读取远端、不产生恢复证明。
            let Some(candidate) = slot.send.as_ref() else { continue };
            if candidate.world_id != world_id {
                continue;
            }
            let step = match candidate.operation.as_str() {
                "send" => {
                    let Some(entries) = entries.as_deref() else { continue };
                    recovery_budget -= 1;
                    if let Some(expectation) = candidate.send_expectation.as_ref() {
                        let mut step = self
                            .recover_frozen_send(
                                world_id,
                                candidate,
                                expectation,
                                entries,
                                candidates,
                                deadline,
                            )
                            .await?;
                        if !step.recovered {
                            self.sight_settlement_reference(
                                world_id,
                                candidate,
                                expectation,
                                entries,
                                deadline,
                                &mut step,
                            )
                            .await?;
                        }
                        step
                    } else {
                        match candidate.origin_kind.as_str() {
                            "settlement" => {
                                self.recover_settlement(world_id, candidate, entries).await?
                            }
                            "mail_intake" => {
                                self.recover_notice(world_id, candidate, entries).await?
                            }
                            _ => continue,
                        }
                    }
                }
                "take" if candidate.origin_kind == "mail_intake" => {
                    recovery_budget -= 1;
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
        Ok((observed, recovered, attempted, matched_count, failed, state_error, requeued))
    }

    /// 所有来源都比对原 sending 尝试，不读取当前模板、key 或业务正文来猜测历史发送。
    async fn recover_frozen_send(
        &self,
        world_id: &str,
        candidate: &MoogleMailboxUnprovenSend,
        expectation: &DeliverySendExpectation,
        entries: &[MoogleMailboxSentEntry],
        candidates: &[MoogleMailboxSweepCandidate],
        deadline: Instant,
    ) -> Result<RecoveryStep, ApplicationError> {
        if !expectation.validate()
            || expectation.world_id != world_id
            || expectation.recipient != candidate.recipient
            || expectation.credential.is_none()
            || candidate.sending_fencing_version <= 0
            || candidate.sending_fencing_version > candidate.fencing_version
        {
            return Ok(RecoveryStep::default());
        }
        let hits = entries
            .iter()
            .filter(|entry| frozen_entry_matches(candidate, expectation, entry, self.clock.now()))
            .take(EXACT_MATCH_CANDIDATES + 1)
            .collect::<Vec<_>>();
        if hits.is_empty() || hits.len() > EXACT_MATCH_CANDIDATES {
            return Ok(RecoveryStep::default());
        }
        let mut step = RecoveryStep { matched: true, ..RecoveryStep::default() };
        let mut exact = None;
        for entry in hits {
            check_deadline(deadline)?;
            let Some(state) = self.sent_state(world_id, entry, &mut step).await? else {
                // 任何竞争详情不可读，都无法证明剩余候选唯一；不跳过失败后挑首个成功。
                return Ok(step);
            };
            if state.subject != expectation.subject || state.body_digest != expectation.body_digest
            {
                continue;
            }
            if !frozen_state_matches(expectation, &state)
                || (!candidate.reference.is_empty()
                    && !state.subject.contains(&candidate.reference))
            {
                // 同内容的资产差异可能是原邮件已被领取；不能跳过它后把另一封当作唯一发送。
                return Ok(step);
            }
            if state.external_mail_id != entry.external_mail_id || exact.is_some() {
                return Ok(step);
            }
            exact = Some((entry, state));
        }
        let Some((entry, state)) = exact else { return Ok(step) };
        if candidates.iter().filter_map(|slot| slot.send.as_ref()).any(|other| {
            other.delivery_id != candidate.delivery_id
                && other.operation == "send"
                && other.send_expectation.as_ref().is_some_and(|other_expectation| {
                    same_send_content(expectation, other_expectation)
                        && frozen_entry_matches(other, other_expectation, entry, self.clock.now())
                })
        }) {
            return Ok(step);
        }
        self.record_observation(candidate, &state).await?;
        step.observed = true;
        step.recovered = self
            .store
            .bind_recovered_send(
                candidate.delivery_id,
                candidate.fencing_version,
                state.external_mail_id.clone(),
                json!({
                    "observed_at": self.clock.now(),
                    "sent_at": entry.sent_at,
                    "operation": "send",
                    "proof": "sent_recovered_frozen_v1",
                    "external_reference_proven": true,
                    "sent_recovered": true,
                    "recovered_mid": state.external_mail_id,
                    "collected": state.collected,
                    "cod_pending": state.cod_pending,
                    "recipient": expectation.recipient,
                    "template": candidate.template,
                    "sending_fencing_version": candidate.sending_fencing_version,
                    "observation_credential": state.observation_credential,
                }),
                Some(expectation),
            )
            .await?;
        Ok(step)
    }

    /// 结算主题带唯一义务 reference；冻结匹配未找回时，在完整 Sent 列表中不限时间寻找同主题邮件。
    /// 命中只记录目击，供 Admin 阻止重试或判失败并转人工 Sent 证明；目击不构成发送证明，
    /// 也不推进投递、资金或履约。列表同前缀条目过多时保守放弃，不读取无界详情。
    async fn sight_settlement_reference(
        &self,
        world_id: &str,
        candidate: &MoogleMailboxUnprovenSend,
        expectation: &DeliverySendExpectation,
        entries: &[MoogleMailboxSentEntry],
        deadline: Instant,
        step: &mut RecoveryStep,
    ) -> Result<(), ApplicationError> {
        if candidate.origin_kind != "settlement"
            || candidate.reference.is_empty()
            || !expectation.subject.contains(&candidate.reference)
        {
            return Ok(());
        }
        let now = self.clock.now();
        let hits = entries
            .iter()
            .filter(|entry| {
                mail_recipient_is_list_projection(&entry.recipient, &candidate.recipient)
                    && notice_subject_matches(&expectation.subject, &entry.subject)
                    && entry.sent_at <= now
            })
            .take(SIGHTING_CANDIDATES + 1)
            .collect::<Vec<_>>();
        if hits.is_empty() || hits.len() > SIGHTING_CANDIDATES {
            return Ok(());
        }
        for entry in hits {
            check_deadline(deadline)?;
            let Some(state) = self.sent_state(world_id, entry, step).await? else {
                return Ok(());
            };
            if state.external_mail_id == entry.external_mail_id
                && state.recipient.eq_ignore_ascii_case(&candidate.recipient)
                && state.subject == expectation.subject
            {
                return self
                    .store
                    .record_sent_sighting(
                        candidate.delivery_id,
                        candidate.fencing_version,
                        &state.external_mail_id,
                        json!({
                            "observed_at": now,
                            "mid": state.external_mail_id,
                            "sent_at": entry.sent_at,
                            "sent_time_precision": "minute",
                            // 详情页安全摘要：Admin 可据此核对人工证明身份，但不接触正文。
                            "recipient": state.recipient,
                            "subject": state.subject,
                            "body_digest": state.body_digest,
                            "collected": state.collected,
                            "cod_pending": state.cod_pending,
                            "remaining_credits": state.remaining_credits,
                            "remaining_equipment_count": state.remaining_equipment_ids.len(),
                        }),
                    )
                    .await;
            }
        }
        Ok(())
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
        let mut hits = entries.iter().filter(|entry| sent_matches(candidate, entry));
        let Some(entry) = hits.next() else { return Ok(RecoveryStep::default()) };
        if hits.next().is_some() {
            return Ok(RecoveryStep::default());
        }
        let mut step = RecoveryStep { matched: true, ..RecoveryStep::default() };
        let Some(state) = self.sent_state(world_id, entry, &mut step).await? else {
            return Ok(step);
        };
        // 列表标题被截断，详情完整标题必须再次包含完整 reference。
        if state.external_mail_id != entry.external_mail_id
            || !state.recipient.eq_ignore_ascii_case(&candidate.recipient)
            || !state.subject.contains(&candidate.reference)
        {
            return Ok(step);
        }
        self.record_observation(candidate, &state).await?;
        step.observed = true;
        if !recovered_state_matches(candidate, &state) {
            return Ok(step);
        }
        // 恢复证明只覆盖身份与金额/资产矩阵；实时发送仍必须走 exact payload 证明。
        step.recovered = self
            .store
            .bind_recovered_send(
                candidate.delivery_id,
                candidate.fencing_version,
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
                None,
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
        if candidate.user_key_user_id.is_some()
            || candidate.credits != 0
            || candidate.cod != 0
            || !candidate.attachment_ids.is_empty()
        {
            return Ok(RecoveryStep::default());
        }
        let expectation = match self.maintenance.notice_expectation(candidate).await {
            Ok(Some(expectation)) => expectation,
            Ok(None) => return Ok(RecoveryStep::default()),
            Err(error) => {
                record_error(
                    DiagnosticLevel::Warn,
                    error.diagnostic_kind(),
                    "mail_maintenance",
                    "read_notice_expectation",
                    DiagnosticContext {
                        world: diagnostic_world(world_id),
                        disposition: Some(DiagnosticDisposition::Unpersisted),
                        ..Default::default()
                    },
                );
                return Ok(RecoveryStep {
                    failed: true,
                    error: Some(error.to_string()),
                    ..RecoveryStep::default()
                });
            }
        };
        let mut hits = entries.iter().filter(|entry| {
            mail_recipient_is_list_projection(&entry.recipient, &candidate.recipient)
                && notice_subject_matches(&expectation.subject, &entry.subject)
                // 页面时间仅精确到分钟；不向下取整 fence，不把更早同分钟邮件当作本次发送。
                // 模板原入站关联由 expectation owner 保证，时间窗本身不是发送身份。
                && entry.sent_at >= candidate.sending_at
                && entry.sent_at <= candidate.outcome_recorded_at
                && entry.sent_at <= self.clock.now()
        });
        let Some(entry) = hits.next() else { return Ok(RecoveryStep::default()) };
        if hits.next().is_some() {
            return Ok(RecoveryStep::default());
        }
        let mut step = RecoveryStep { matched: true, ..RecoveryStep::default() };
        let Some(state) = self.sent_state(world_id, entry, &mut step).await? else {
            return Ok(step);
        };
        if state.external_mail_id != entry.external_mail_id
            || !state.recipient.eq_ignore_ascii_case(&candidate.recipient)
            || state.subject != expectation.subject
            || state.body_digest != expectation.body_digest
            || state.remaining_credits != 0
            || state.cod_pending != 0
            || !state.remaining_equipment_ids.is_empty()
        {
            return Ok(step);
        }
        self.record_observation(candidate, &state).await?;
        step.observed = true;
        step.recovered = self
            .store
            .bind_recovered_send(
                candidate.delivery_id,
                candidate.fencing_version,
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
                None,
            )
            .await?;
        Ok(step)
    }

    /// Take 只解释原账号下的明确缺失或完整资产观察；旧无绑定记录保留人工核对。
    async fn recover_take(
        &self,
        world_id: &str,
        candidate: &MoogleMailboxUnprovenSend,
    ) -> Result<RecoveryStep, ApplicationError> {
        let Some(expectation) = candidate.take_expectation.as_ref() else {
            return Ok(RecoveryStep::default());
        };
        if !expectation.validate()
            || expectation.world_id != world_id
            || expectation.world_id != candidate.world_id
            || expectation.external_mail_id != candidate.inbound_mail_id
            || candidate.sending_fencing_version <= 0
            || candidate.sending_fencing_version > candidate.fencing_version
        {
            return Ok(RecoveryStep::default());
        }
        let Some(expected_credential) = expectation.credential.as_ref() else {
            return Ok(RecoveryStep::default());
        };
        let observed =
            match self.maintenance.inbox_assets(world_id, &candidate.inbound_mail_id).await {
                Ok(value) => value,
                Err(error) => {
                    record_error(
                        DiagnosticLevel::Warn,
                        error.diagnostic_kind(),
                        "mail_maintenance",
                        "read_inbox_assets",
                        DiagnosticContext {
                            world: diagnostic_world(world_id),
                            disposition: Some(DiagnosticDisposition::Unpersisted),
                            ..Default::default()
                        },
                    );
                    return Ok(RecoveryStep {
                        failed: true,
                        error: Some(error.to_string()),
                        ..RecoveryStep::default()
                    });
                }
            };
        let Some(observation) = observed.observation_credential.as_ref() else {
            return Ok(RecoveryStep::default());
        };
        // 重启可推进 generation，但不能换凭据或版本；本次 generation 仍由存储锁内复核。
        if !observation.validate()
            || observation.credential_id != expected_credential.credential_id
            || observation.credential_version != expected_credential.credential_version
        {
            return Ok(RecoveryStep::default());
        }
        match observed.assets {
            None => self.bind_take(candidate, expectation, observation, "inbox_mail_absent").await,
            Some(present) => {
                if !present.sender.eq_ignore_ascii_case(&candidate.recipient) {
                    return Ok(RecoveryStep::default());
                }
                if present.equipment_ids.is_empty() && present.credits == 0 && present.cod == 0 {
                    self.bind_take(candidate, expectation, observation, "attachments_absent").await
                } else if take_assets_match(candidate, &present) {
                    self.requeue(candidate, expectation, observation, "take_assets_still_present")
                        .await
                } else {
                    Ok(RecoveryStep::default())
                }
            }
        }
    }

    async fn bind_take(
        &self,
        candidate: &MoogleMailboxUnprovenSend,
        expectation: &DeliveryTakeExpectation,
        observation: &DeliverySendCredentialBinding,
        reason: &str,
    ) -> Result<RecoveryStep, ApplicationError> {
        let recovered = self
            .store
            .bind_recovered_action(
                candidate.delivery_id,
                candidate.fencing_version,
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
                expectation,
                observation,
            )
            .await?;
        Ok(RecoveryStep { recovered, ..RecoveryStep::default() })
    }

    async fn requeue(
        &self,
        candidate: &MoogleMailboxUnprovenSend,
        expectation: &DeliveryTakeExpectation,
        observation: &DeliverySendCredentialBinding,
        reason: &str,
    ) -> Result<RecoveryStep, ApplicationError> {
        if candidate.sweep_retries >= MAX_SWEEP_RETRIES {
            return Ok(RecoveryStep::default());
        }
        let requeued = self
            .store
            .requeue_unproven(
                candidate.delivery_id,
                candidate.fencing_version,
                candidate.sweep_retries + 1,
                reason,
                expectation,
                observation,
            )
            .await?;
        Ok(RecoveryStep { requeued, ..RecoveryStep::default() })
    }

    /// 记录 Sent 详情的观测快照；恢复证明是否成立由调用方随后判定。
    async fn record_observation(
        &self,
        candidate: &MoogleMailboxUnprovenSend,
        state: &MoogleMailboxSentState,
    ) -> Result<(), ApplicationError> {
        self.store
            .record_sent_observation(
                candidate.delivery_id,
                candidate.fencing_version,
                json!({
                    "observed_at": self.clock.now(),
                    "mid": state.external_mail_id,
                    "recipient": state.recipient,
                    "subject": state.subject,
                    "body_digest": state.body_digest,
                    "collected": state.collected,
                    "cod_pending": state.cod_pending,
                    "remaining_credits": state.remaining_credits,
                    "remaining_equipment_count": state.remaining_equipment_ids.len(),
                }),
            )
            .await
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
                record_error(
                    DiagnosticLevel::Warn,
                    error.diagnostic_kind(),
                    "mail_maintenance",
                    "read_sent_state",
                    DiagnosticContext {
                        world: diagnostic_world(world_id),
                        disposition: Some(DiagnosticDisposition::Unpersisted),
                        ..Default::default()
                    },
                );
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

fn frozen_entry_matches(
    candidate: &MoogleMailboxUnprovenSend,
    expectation: &DeliverySendExpectation,
    entry: &MoogleMailboxSentEntry,
    now: chrono::DateTime<chrono::Utc>,
) -> bool {
    candidate.world_id == expectation.world_id
        && mail_recipient_is_list_projection(&entry.recipient, &expectation.recipient)
        && notice_subject_matches(&expectation.subject, &entry.subject)
        // 列表时间仅精确到分钟，发送通常在开始的同一分钟完成，开始围栏按分钟起点比较。
        // 同分钟更早的同内容邮件由详情唯一匹配、MID 绑定唯一和存储侧整分钟歧义检查排除。
        && entry.sent_at >= sent_list_minute_start(candidate.sending_at)
        && entry.sent_at <= candidate.outcome_recorded_at
        && entry.sent_at <= now
}

fn frozen_state_matches(
    expectation: &DeliverySendExpectation,
    state: &MoogleMailboxSentState,
) -> bool {
    let (Some(expected), Some(observed)) = (&expectation.credential, &state.observation_credential)
    else {
        return false;
    };
    // 重启可改变当前 owner generation；原凭据 ID/version 不得改变，当前 generation 由存储锁验。
    expected.credential_id == observed.credential_id
        && expected.credential_version == observed.credential_version
        && state.recipient.eq_ignore_ascii_case(&expectation.recipient)
        && state.subject == expectation.subject
        && state.body_digest == expectation.body_digest
        && state.cod_pending == expectation.cod
        && state.remaining_credits == expectation.credits
        && state.remaining_equipment_ids == expectation.equipment_ids
}

fn same_send_content(left: &DeliverySendExpectation, right: &DeliverySendExpectation) -> bool {
    let same_credential = match (&left.credential, &right.credential) {
        (Some(left), Some(right)) => {
            left.credential_id == right.credential_id
                && left.credential_version == right.credential_version
        }
        _ => false,
    };
    same_credential
        && left.world_id == right.world_id
        && left.recipient.eq_ignore_ascii_case(&right.recipient)
        && left.subject == right.subject
        && left.body_digest == right.body_digest
        && left.credits == right.credits
        && left.cod == right.cod
        && left.equipment_ids == right.equipment_ids
}

/// 恢复仅接受尚未领取的完整冻结资产矩阵；领取后原 Credits/COD 已不可从剩余值证明。
fn recovered_state_matches(
    candidate: &MoogleMailboxUnprovenSend,
    state: &MoogleMailboxSentState,
) -> bool {
    if !state.recipient.eq_ignore_ascii_case(&candidate.recipient)
        || state.collected
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
    mail_recipient_is_list_projection(&entry.recipient, &candidate.recipient)
        && reference_is_subject_projection(&entry.subject, &candidate.reference)
}

fn reference_is_subject_projection(subject: &str, reference: &str) -> bool {
    let Some(start) = subject.find('[') else { return false };
    let tail = &subject[start + 1..];
    let prefix = tail.split(']').next().unwrap_or(tail).trim();
    prefix.len() >= 6 && reference.starts_with(prefix)
}

/// 重排只接受未发生部分领取的原完整资产；集合比较不依赖远端顺序，也不接受重复 ID。
fn take_assets_match(
    candidate: &MoogleMailboxUnprovenSend,
    present: &MoogleMailboxInboxAssets,
) -> bool {
    if candidate.attachment_ids.is_empty()
        || candidate.attachment_ids.len() != present.equipment_ids.len()
        || candidate.credits != present.credits
        || candidate.cod != present.cod
    {
        return false;
    }
    let mut expected = candidate.attachment_ids.iter().collect::<std::collections::HashSet<_>>();
    expected.len() == candidate.attachment_ids.len()
        && present.equipment_ids.iter().all(|id| expected.remove(id))
}

fn compose_matches(
    candidate: &MoogleMailboxUnprovenSend,
    compose: &MoogleMailboxComposeState,
) -> bool {
    if candidate.operation != "send" {
        return false;
    }
    if let Some(expectation) = &candidate.send_expectation {
        let (Some(expected), Some(observed)) =
            (&expectation.credential, &compose.observation_credential)
        else {
            return false;
        };
        return expectation.validate()
            && expectation.world_id == candidate.world_id
            && expected.credential_id == observed.credential_id
            && expected.credential_version == observed.credential_version
            && expectation.recipient.eq_ignore_ascii_case(&compose.recipient)
            && expectation.subject == compose.subject
            && expectation.body_digest == compose.body_digest
            && compose.cod == Some(expectation.cod)
            && compose.credits == expectation.credits
            && compose.equipment_ids == expectation.equipment_ids;
    }
    // 通知类没有业务 reference；空串会匹配任意草稿，必须显式排除。
    // 旧行也必须核对原装备身份；总附件数还包含 Credits，不能代替 EID 证明。
    !candidate.reference.is_empty()
        && candidate.recipient.eq_ignore_ascii_case(&compose.recipient)
        && compose.cod == Some(candidate.cod)
        && compose.credits == candidate.credits
        && candidate.attachment_ids.windows(2).all(|ids| ids[0] < ids[1])
        && compose.equipment_ids == candidate.attachment_ids
        && (compose.subject.contains(&candidate.reference)
            || compose.body.contains(&candidate.reference))
}

fn check_deadline(deadline: Instant) -> Result<(), ApplicationError> {
    if Instant::now() >= deadline {
        return Err(repository_error("MoogleMail 维护总预算耗尽"));
    }
    Ok(())
}

fn repository_error(message: &str) -> ApplicationError {
    RepositoryError::Failed(message.to_owned()).into()
}
