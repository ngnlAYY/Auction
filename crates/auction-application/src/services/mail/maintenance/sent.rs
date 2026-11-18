//! Sent/Take 恢复与 Read Mail 观察：只按原发送尝试的冻结事实比对，找不到不证明未发送。

use std::time::Instant;

use auction_domain::SettlementReturnTakeUnknownReason;
use serde_json::json;

use super::MailboxMaintenanceService;
use super::diagnostics::{check_deadline, record_unpersisted_error, repository_error};
use super::matching::{
    frozen_entry_matches, frozen_state_matches, notice_subject_matches, recovered_state_matches,
    return_take_assets_match, same_send_content, sent_matches, take_assets_match,
};
use super::sweep::sent_metadata;
use crate::errors::ApplicationError;
use crate::ports::delivery::{
    DeliverySendCredentialBinding, DeliverySendExpectation, DeliveryTakeExpectation,
};
use crate::ports::mail::{
    MoogleMailboxReturnTake, MoogleMailboxSentEntry, MoogleMailboxSentState,
    MoogleMailboxSweepCandidate, MoogleMailboxUnprovenSend, mail_recipient_is_list_projection,
};

/// 相同列表投影过多时保留未知，不读取无界详情或只选择前几个结果冒充唯一性。
const EXACT_MATCH_CANDIDATES: usize = 32;
/// 单条结算投递在完整 Sent 中按同主题前缀最多读取的详情数；超出即保守放弃目击。
const SIGHTING_CANDIDATES: usize = 4;
/// 单条投递允许的维护重排队次数；达到上限后完整资产仍在时不再自动重排、保留人工核对，防止
/// 无限重试。上限只约束重排：其后观察到领取完成证明仍可收口；结算退件 Take 另把“完整仍在且
/// 可领取”的观察持久化，供 Admin 以新命令再次领回。
const MAX_SWEEP_RETRIES: i64 = 2;

/// 结算退件 Take 的巡检完成原因：integrations 已核对原身份、原正文前缀与领取回执。存储只接受
/// 这一原因，“附件为空”本身不再是结算退件的完成证明。
const RETURN_TAKE_RECEIPT_REASON: &str = "take_receipt_proven";

impl MailboxMaintenanceService {
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

    /// 所有来源都比对原 sending 尝试，不读取当前模板、key 或业务正文来猜测历史发送。
    pub(super) async fn recover_frozen_send(
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
        step.observed = self.record_observation(candidate, &state).await?;
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
    pub(super) async fn sight_settlement_reference(
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
                        sent_metadata(&state, now, Some(entry.sent_at)),
                    )
                    .await
                    .map(|_| ());
            }
        }
        Ok(())
    }

    /// 结算类：标题携带义务 reference，详情必须仍带对应装备或 carrier。
    pub(super) async fn recover_settlement(
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
        step.observed = self.record_observation(candidate, &state).await?;
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
    pub(super) async fn recover_notice(
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
                record_unpersisted_error("read_notice_expectation", world_id, &error);
                return Ok(RecoveryStep { failed: true, ..RecoveryStep::default() });
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
        step.observed = self.record_observation(candidate, &state).await?;
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
    ///
    /// 结算退件 Take（`settlement_return_action`）会把领回证明变成资金返还，口径更严：完成证明
    /// 只认 integrations 在原 Take 会话里核对过原身份、原正文前缀与逐条领取回执的结论（附带预期
    /// 资产，由存储锁内逐项比对），附件为空而回执不成立仍是未知；原完整资产仍在才重新排队；邮件
    /// 整体缺失可能是过期或删除，不能证明资产入账，保持结果未知。
    pub(super) async fn recover_take(
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
        if candidate.origin_kind == "settlement_return_action" {
            return self
                .recover_return_take(world_id, candidate, expectation, expected_credential)
                .await;
        }
        let observed =
            match self.maintenance.inbox_assets(world_id, &candidate.inbound_mail_id).await {
                Ok(value) => value,
                Err(error) => {
                    record_unpersisted_error("read_inbox_assets", world_id, &error);
                    return Ok(RecoveryStep { failed: true, ..RecoveryStep::default() });
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

    /// 结算退件 Take：只有 integrations 给出的领取完成结论才绑定证明；原完整资产仍在则重新排队；
    /// 缺失、回执不成立或部分取走都保持未知。证据缺少系统后缀事实时没有可核对的原身份，不读远端。
    /// 不推进案件的观察（重排已用满、资产不符、邮件缺失、回执不成立）以原因闭集持久化到案件
    /// 事件，详情与告警据此说明；换账号或换凭据版本的观察不是原会话，不留证。
    async fn recover_return_take(
        &self,
        world_id: &str,
        candidate: &MoogleMailboxUnprovenSend,
        expectation: &DeliveryTakeExpectation,
        expected_credential: &DeliverySendCredentialBinding,
    ) -> Result<RecoveryStep, ApplicationError> {
        let Some(identity) = candidate.return_take.as_ref() else {
            return Ok(RecoveryStep::default());
        };
        let observed = match self
            .maintenance
            .return_take_receipt(world_id, &candidate.inbound_mail_id, identity)
            .await
        {
            Ok(value) => value,
            Err(error) => {
                record_unpersisted_error("read_return_take_receipt", world_id, &error);
                return Ok(RecoveryStep { failed: true, ..RecoveryStep::default() });
            }
        };
        let Some(observation) = observed.observation_credential.as_ref() else {
            return Ok(RecoveryStep::default());
        };
        if !observation.validate()
            || observation.credential_id != expected_credential.credential_id
            || observation.credential_version != expected_credential.credential_version
        {
            return Ok(RecoveryStep::default());
        }
        let reason = match observed.outcome {
            MoogleMailboxReturnTake::Taken => {
                return self
                    .bind_take(candidate, expectation, observation, RETURN_TAKE_RECEIPT_REASON)
                    .await;
            }
            MoogleMailboxReturnTake::Present { assets, claimable }
                if assets.sender.eq_ignore_ascii_case(&candidate.recipient)
                    && return_take_assets_match(candidate, &assets) =>
            {
                if candidate.sweep_retries < MAX_SWEEP_RETRIES {
                    return self
                        .requeue(candidate, expectation, observation, "take_assets_still_present")
                        .await;
                }
                // 自动重排已用满：完整资产仍在且可领取是 Admin 再次领回的机器证明，否则只说明。
                if claimable {
                    SettlementReturnTakeUnknownReason::AssetsClaimable
                } else {
                    SettlementReturnTakeUnknownReason::AssetsUnclaimable
                }
            }
            MoogleMailboxReturnTake::Present { .. } => {
                SettlementReturnTakeUnknownReason::AssetsChanged
            }
            MoogleMailboxReturnTake::Missing => SettlementReturnTakeUnknownReason::MailMissing,
            MoogleMailboxReturnTake::Unproven => SettlementReturnTakeUnknownReason::ReceiptUnproven,
        };
        self.store
            .record_return_take_observation(
                candidate.delivery_id,
                candidate.fencing_version,
                reason,
                candidate.sweep_retries,
                expectation,
                observation,
            )
            .await?;
        Ok(RecoveryStep::default())
    }

    async fn bind_take(
        &self,
        candidate: &MoogleMailboxUnprovenSend,
        expectation: &DeliveryTakeExpectation,
        observation: &DeliverySendCredentialBinding,
        reason: &str,
    ) -> Result<RecoveryStep, ApplicationError> {
        let mut metadata = json!({
            "observed_at": self.clock.now(),
            "operation": "take",
            "proof": "action_recovered_inbox_v1",
            "external_reference_proven": true,
            "recovered_mid": candidate.inbound_mail_id,
            "reason": reason,
            "attachment_count": candidate.attachment_ids.len(),
        });
        if candidate.origin_kind == "settlement_return_action" {
            // 与成功页回执同一组预期资产字段；存储在锁内与动作冻结事实逐项比对后才记账。
            metadata["expected_credits"] = candidate.credits.into();
            metadata["expected_cod"] = candidate.cod.into();
            metadata["expected_attachment_ids"] = candidate.attachment_ids.clone().into();
            if let Some(identity) = candidate.return_take.as_ref() {
                metadata["receipt_attachment_count"] = identity.attachment_count.into();
            }
        }
        let recovered = self
            .store
            .bind_recovered_action(
                candidate.delivery_id,
                candidate.fencing_version,
                candidate.inbound_mail_id.clone(),
                metadata,
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

    /// 只有当前 fence 接受观察才计数；是否能恢复仍由原证明事务复核（不变量 6）。
    async fn record_observation(
        &self,
        candidate: &MoogleMailboxUnprovenSend,
        state: &MoogleMailboxSentState,
    ) -> Result<bool, ApplicationError> {
        self.store
            .record_sent_observation(
                candidate.delivery_id,
                candidate.fencing_version,
                sent_metadata(state, self.clock.now(), None),
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
                record_unpersisted_error("read_sent_state", world_id, &error);
                step.failed = true;
                Ok(None)
            }
        }
    }
}

/// 单条候选的恢复进度；只用于本轮统计，不进入持久状态。
#[derive(Default)]
pub(super) struct RecoveryStep {
    pub(super) matched: bool,
    pub(super) observed: bool,
    pub(super) recovered: bool,
    pub(super) requeued: bool,
    pub(super) failed: bool,
}

#[cfg(test)]
mod tests {
    use auction_time::Timestamp;
    use serde_json::json;

    use super::super::sweep::sent_metadata;
    use crate::ports::mail::MoogleMailboxSentState;

    #[test]
    fn both_worker_observations_encode_the_complete_seen_manifest() {
        let now = Timestamp::from_ymd_hms(2026, 11, 2, 1, 14, 0).expect("观察时间");
        let mut state = MoogleMailboxSentState {
            external_mail_id: "7000001".to_owned(),
            recipient: "Buyer".to_owned(),
            subject: "Settlement".to_owned(),
            collected: false,
            cod_pending: 0,
            remaining_credits: 0,
            remaining_equipment_ids: ["0100", "20", "100", "2"].map(str::to_owned).to_vec(),
            body_digest: [1; 32],
            observation_credential: None,
        };
        for sent_at in [None, Some(now)] {
            let observed = sent_metadata(&state, now, sent_at);
            assert_eq!(observed["remaining_equipment_ids"], json!(["100", "2", "20"]));
            assert_eq!(observed["remaining_equipment_count"], 3);
            assert_eq!(observed.get("sent_at").is_some(), sent_at.is_some());
        }
        state.collected = true;
        state.remaining_equipment_ids.clear();
        let observed = sent_metadata(&state, now, Some(now));
        assert_eq!(observed["remaining_equipment_ids"], json!([]));
        assert_eq!(observed["remaining_equipment_count"], 0);

        state.collected = false;
        state.remaining_equipment_ids = vec!["0".to_owned()];
        let observed = sent_metadata(&state, now, Some(now));
        assert!(observed["remaining_equipment_ids"].is_null());
        assert_eq!(observed["remaining_equipment_count"], 1);
    }
}
