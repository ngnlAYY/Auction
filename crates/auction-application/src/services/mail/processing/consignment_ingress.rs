//! 寄售来信的 poll 编排：4D1 候选预读、纯分类与同事务提交。
//!
//! 本服务不读取远端、不持有正文或装备 key；首次决定和游标推进都在
//! [`PollEnvelopeStore`] 的同一顶层事务中完成。旧路由必须已在调用方先行。

use std::fmt::Display;
use std::sync::Arc;

use auction_domain::consignment::OriginalMailKey;
use auction_domain::consignment::deposit::{
    EnvelopeObservation, LegacyReturnEvidence, PendingReason,
};
use auction_time::Timestamp;
use uuid::Uuid;

use crate::ports::consignment::{
    ConsignmentError, EnvelopeReceipt, PollEnvelopeStore, PollEnvelopeWrite, PollRawObservation,
};
use crate::ports::mail::{MailMessage, MailPollClaim};
use crate::ports::settlement::MailAccountEvidenceBinding;
use crate::services::mail::processing::consignment::{
    ConsignmentMailInput, ConsignmentMailPlan, ConsignmentMailService,
};

/// 候选输入无效；安全日志只保留错误种类，不回显原标题或正文。
fn invalid_candidate<E: Display>(error: E) -> ConsignmentError {
    tracing::debug!(error = %error, "寄售候选输入校验失败");
    ConsignmentError::InvalidDepositRequest
}

/// 4D1 已认证的寄售候选；字段与网关/Worker 两态消息的安全投影一致。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsignmentMailCandidate {
    /// 收件世界。
    pub world_id: String,
    /// 邮件 MID。
    pub external_mail_id: String,
    /// 完整发件人显示名。
    pub sender: String,
    /// 完整主题；只用于分类与旧路由判据，不进入观察投影。
    pub subject: String,
    /// 列表原始收件分钟。
    pub received_minute_raw: String,
    /// 列表收件时刻。
    pub received_at: Timestamp,
    /// 领域观察。
    pub observation: EnvelopeObservation,
    /// 认证会话绑定。
    pub binding: MailAccountEvidenceBinding,
    /// 远端观察身份。
    pub remote_observation_id: Uuid,
    /// 观察时刻。
    pub observed_at: Timestamp,
    /// 主题摘要。
    pub subject_digest: [u8; 32],
    /// 正文摘要。
    pub body_digest: [u8; 32],
    /// 原始附件数。
    pub raw_count: Option<u32>,
    /// 系统 RTS 的旧退件证据。
    pub legacy_evidence: LegacyReturnEvidence,
}

/// 发件人未证明时分类输入使用的占位值；不会写入任何持久事实。
const UNPROVEN_SENDER: &str = "unproven-sender";

/// 无通用解析结果的候选：以候选安全字段合成只含身份与主题的分类输入（无正文、无附件）。
fn unparsed_candidate_message(
    candidate: &ConsignmentMailCandidate,
    claim: &MailPollClaim,
) -> MailMessage {
    MailMessage {
        mailbox_key: claim.mailbox_key.clone(),
        external_mail_id: candidate.external_mail_id.clone(),
        world_id: candidate.world_id.clone(),
        received_at: candidate.received_at,
        sender: if candidate.sender.is_empty() {
            UNPROVEN_SENDER.to_owned()
        } else {
            candidate.sender.clone()
        },
        subject: candidate.subject.clone(),
        body: String::new(),
        credits: 0,
        cod: 0,
        attachments: Vec::new(),
        returnable: false,
        claimable: false,
        content_digest: candidate.body_digest,
    }
}

/// 一封候选的 poll 上下文。
pub struct ConsignmentIngressRequest<'a> {
    /// 旧路由无法归属的业务 envelope；严格通用解析失败的候选（如重复原文 EID）没有该信，
    /// 为 `None`：不经旧路由，由候选观察自身（发件人可为空）进入分类并落成待隔离决定。
    pub message: Option<&'a MailMessage>,
    /// 同一详情文档上的 4D1 候选。
    pub candidate: &'a ConsignmentMailCandidate,
    /// 当前轮询租约。
    pub claim: &'a MailPollClaim,
    /// 本次轮询时刻。
    pub polled_at: Timestamp,
    /// 下一次调度时刻。
    pub next_poll_at: Timestamp,
}

/// 候选处理结果；`Pending` 与 `NotApplicable` 都不推进游标。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsignmentIngressOutcome {
    /// 分类器仍判定不适用；调用方须继续原 `process_unknown`。
    NotApplicable,
    /// 临时不可读或身份未建立；游标停在候选之前。
    Pending(PendingReason),
    /// 首次决定已提交，游标越过候选并释放租约。
    Committed(EnvelopeReceipt),
    /// 已有决定，仅恢复原回执并推进前缀。
    Recovered(EnvelopeReceipt),
}

/// 寄售候选的 poll 编排服务。
pub struct ConsignmentIngressService {
    mail: Arc<ConsignmentMailService>,
    store: Arc<dyn PollEnvelopeStore>,
}

impl ConsignmentIngressService {
    /// 注入纯分类服务与同一事务存储端口。
    pub fn new(mail: Arc<ConsignmentMailService>, store: Arc<dyn PollEnvelopeStore>) -> Self {
        Self { mail, store }
    }

    /// 预读、分类并在寄售事务内提交首次决定或恢复既有决定。
    ///
    /// 不变量 1、3、4、6、8：旧路由由调用方先行；首次观察、决定、回执、待读前缀与游标
    /// 同事务提交；pending 不写终局，恢复不重写首次事实。
    pub async fn process(
        &self,
        request: ConsignmentIngressRequest<'_>,
    ) -> Result<ConsignmentIngressOutcome, ConsignmentError> {
        let candidate = request.candidate;
        // 无通用解析结果时只用候选自带的安全字段合成分类输入；发件人未证明时用占位值，
        // 占位值只参与主题/发件人文本判据，不进入卖家预读与原始观察（见下方 `sender`）。
        let unparsed_shell;
        let message = match request.message {
            Some(message) => message,
            None => {
                unparsed_shell = unparsed_candidate_message(candidate, request.claim);
                &unparsed_shell
            }
        };
        message.validate().map_err(invalid_candidate)?;
        // 发件人只采信详情完整发件人；无通用解析结果时允许“未证明”，其观察已标记列表/详情不一致。
        let sender = (!candidate.sender.is_empty()).then(|| candidate.sender.clone());
        if candidate.world_id != message.world_id
            || candidate.external_mail_id != message.external_mail_id
            || sender.is_none() && request.message.is_some()
            || candidate.subject.is_empty()
            || candidate.observation.observation_id.is_nil()
            || candidate.remote_observation_id.is_nil()
            || !candidate.binding.validate()
            || candidate.binding.world.as_str() != message.world_id
            || candidate.binding.authenticated_uid.is_empty()
            || candidate.binding.digest != candidate.observation.session_binding_digest
            || request.claim.mailbox_key != message.mailbox_key
        {
            return Err(ConsignmentError::InvalidDepositRequest);
        }
        let world = auction_domain::consignment::parse_settlement_world(&candidate.world_id)
            .ok_or(ConsignmentError::InvalidDepositRequest)?;
        let key = OriginalMailKey::new(
            &candidate.binding.authenticated_uid,
            world,
            &candidate.external_mail_id,
        )
        .map_err(invalid_candidate)?;
        let preflight = self
            .store
            .load_envelope_preflight(key.clone(), candidate.observation.clone(), sender.clone())
            .await?;
        let plan = self
            .mail
            .plan(ConsignmentMailInput {
                message,
                mail_key: Some(key.clone()),
                observation: candidate.observation.clone(),
                reservations: preflight.reservations,
                envelope_seller_user_id: preflight.seller_user_id,
                capabilities: preflight.capabilities,
                read_failure: false,
                existing_decision: preflight.existing_decision.clone(),
                now: preflight.now,
            })
            .await?;
        match plan {
            ConsignmentMailPlan::NotApplicable(_) => Ok(ConsignmentIngressOutcome::NotApplicable),
            ConsignmentMailPlan::Pending(reason) => Ok(ConsignmentIngressOutcome::Pending(reason)),
            ConsignmentMailPlan::Recovered(receipt) => {
                let write = PollEnvelopeWrite {
                    mail_key: key,
                    command_id: receipt.command_id,
                    command: None,
                    raw: None,
                    claim: request.claim.clone(),
                    polled_at: request.polled_at,
                    next_poll_at: request.next_poll_at,
                    cursor_mid: candidate.external_mail_id.clone(),
                };
                let committed = self.store.decide_envelope_in_poll(write).await?;
                if committed != receipt {
                    return Err(ConsignmentError::Unavailable);
                }
                Ok(ConsignmentIngressOutcome::Recovered(receipt))
            }
            ConsignmentMailPlan::First(command) => {
                let command_id = command.command_id;
                let raw = PollRawObservation {
                    mail_key: key.clone(),
                    session_id: preflight.session_id,
                    mailbox_key: message.mailbox_key.clone(),
                    remote_observation_id: candidate.remote_observation_id.to_string(),
                    observed_at: candidate.observed_at,
                    parser_version: candidate.observation.parser_version.clone(),
                    evidence_ref: None,
                    subject_digest: candidate.subject_digest,
                    body_digest: candidate.body_digest,
                    sender,
                    recipient: None,
                    raw_count: candidate.raw_count,
                    observation: candidate.observation.clone(),
                    legacy_evidence: candidate.legacy_evidence.clone(),
                    received_minute_raw: candidate.received_minute_raw.clone(),
                    received_timezone: "UTC".to_owned(),
                    first_seen_at: candidate.received_at,
                    season_window_id: preflight.season_window_id,
                };
                let write = PollEnvelopeWrite {
                    mail_key: key,
                    command_id,
                    command: Some(*command),
                    raw: Some(raw),
                    claim: request.claim.clone(),
                    polled_at: request.polled_at,
                    next_poll_at: request.next_poll_at,
                    cursor_mid: candidate.external_mail_id.clone(),
                };
                let receipt = self.store.decide_envelope_in_poll(write).await?;
                Ok(ConsignmentIngressOutcome::Committed(receipt))
            }
        }
    }
}
