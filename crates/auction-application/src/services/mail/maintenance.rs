//! MoogleMail 三页定期维护：Sent 观察留证，以及仅针对精确匹配投递的草稿 discard。
//!
//! Sent 证据只有唯一、精确匹配时才把历史 `write_unknown` 投递绑定回真实 MID，并重新排队
//! 等待收取证据；证明标记为恢复证明，不冒充实时发送证明。compose 草稿只有在收件人、
//! 业务 reference 和 COD 同时匹配唯一一条未证明投递时才丢弃。

use std::sync::Arc;

use serde_json::json;
use uuid::Uuid;

use crate::{
    ApplicationError, Clock, MoogleMailboxComposeState, MoogleMailboxDiscard,
    MoogleMailboxEvidenceStore, MoogleMailboxMaintenance, MoogleMailboxSentEntry,
    MoogleMailboxUnprovenSend, RepositoryError,
};

/// 单轮维护结果，只包含安全统计，不含邮件正文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailboxMaintenanceReport {
    pub compose_clean: bool,
    pub matched_delivery: Option<Uuid>,
    pub discarded: bool,
    pub compose_matches: usize,
    pub sent_observed: usize,
    pub sent_recovered: usize,
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
        let (sent_observed, sent_recovered) = self.recover_sent(world_id, &candidates).await?;
        let compose = self.maintenance.compose(world_id).await?;
        if compose.clean {
            return Ok(MailboxMaintenanceReport {
                compose_clean: true,
                matched_delivery: None,
                discarded: false,
                compose_matches: 0,
                sent_observed,
                sent_recovered,
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
        })
    }

    async fn recover_sent(
        &self,
        world_id: &str,
        candidates: &[MoogleMailboxUnprovenSend],
    ) -> Result<(usize, usize), ApplicationError> {
        if candidates.is_empty() {
            // 没有未证明投递时仍完整读取一次 Sent，保持三页周期读取。
            self.maintenance.sent_entries(world_id).await?;
            return Ok((0, 0));
        }
        let entries = self.maintenance.sent_entries(world_id).await?;
        let mut observed = 0;
        let mut recovered = 0;
        for candidate in candidates {
            let matched =
                entries.iter().filter(|entry| sent_matches(candidate, entry)).collect::<Vec<_>>();
            if matched.len() != 1 {
                continue;
            }
            let entry = matched[0];
            let Ok(state) = self.maintenance.sent_state(world_id, entry, self.clock.now()).await
            else {
                continue;
            };
            let observation = json!({
                "observed_at": self.clock.now(),
                "mid": state.external_mail_id,
                "collected": state.collected,
                "cod_pending": state.cod_pending,
                "remaining_credits": state.remaining_credits,
                "remaining_equipment_count": state.remaining_equipment_ids.len(),
            });
            self.store.record_sent_observation(candidate.delivery_id, observation.clone()).await?;
            observed += 1;
            if !recovered_state_matches(candidate, &state) {
                continue;
            }
            // 恢复证明只覆盖身份与金额/资产矩阵；实时发送仍必须走 exact payload 证明。
            let metadata = json!({
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
            });
            self.store
                .bind_recovered_send(candidate.delivery_id, state.external_mail_id, metadata)
                .await?;
            recovered += 1;
        }
        Ok((observed, recovered))
    }
}

/// 收集状态必须与投递载荷一致：carrier 允许 1C，其余发送必须仍带那件装备。
fn recovered_state_matches(
    candidate: &MoogleMailboxUnprovenSend,
    state: &crate::MoogleMailboxSentState,
) -> bool {
    if state.collected {
        return candidate.cod == 0 || state.cod_pending == 0;
    }
    if state.cod_pending != candidate.cod {
        return false;
    }
    if candidate.template == "settlement_payment_carrier" {
        state.remaining_credits == 1 && state.remaining_equipment_ids.is_empty()
    } else {
        state.remaining_credits == 0 && state.remaining_equipment_ids.len() == 1
    }
}

fn sent_matches(candidate: &MoogleMailboxUnprovenSend, entry: &MoogleMailboxSentEntry) -> bool {
    candidate.recipient.eq_ignore_ascii_case(&entry.recipient)
        && entry.subject.contains(&candidate.reference)
}

fn compose_matches(
    candidate: &MoogleMailboxUnprovenSend,
    compose: &MoogleMailboxComposeState,
) -> bool {
    candidate.recipient.eq_ignore_ascii_case(&compose.recipient)
        && candidate.cod == compose.cod.unwrap_or(0)
        && (compose.subject.contains(&candidate.reference)
            || compose.body.contains(&candidate.reference))
}

fn repository_error(message: &str) -> ApplicationError {
    RepositoryError::Failed(message.to_owned()).into()
}
