//! 定期维护适配：复用 delivery transport 的凭据集合与发送器读取三页。
//!
//! 普通邮箱读取不占 compose guard；只有 compose 观察和 discard 持有短时排他会话。
//! 读取接受前重新核验当前凭据与 owner，不把共享观察升级成资金或库存授权。

use auction_application::core::diagnostics::{DiagnosticContext, diagnostic_world};

use crate::safe_diagnostics;

use auction_application::core::{ApplicationError, RepositoryError};
use auction_application::mail::{
    MailboxFuture, MoogleMailboxComposeState, MoogleMailboxDiscard, MoogleMailboxInboxAssets,
    MoogleMailboxInboxObservation, MoogleMailboxMaintenance, MoogleMailboxNoticeExpectation,
    MoogleMailboxSentEntries, MoogleMailboxSentEntry, MoogleMailboxSentState,
    MoogleMailboxUnprovenSend,
};
use chrono::{DateTime, Utc};

use super::{
    super::{
        MoogleMailDiscardOutcome, MoogleMailSentListEntry, parser::canonical_mail_body_digest,
    },
    *,
};

/// 三页维护的远端适配器；不包含投递 claim、租约或重试状态。
pub struct MoogleMailMaintenanceAdapter {
    credentials: MoogleMailCredentialSet,
    sender: MoogleMailSender,
    templates: MoogleMailNotificationTemplates,
}

impl MoogleMailDeliveryTransport {
    /// 从当前 transport 复制凭据集合与发送器，供维护循环独立持有。
    pub fn maintenance_adapter(&self) -> MoogleMailMaintenanceAdapter {
        MoogleMailMaintenanceAdapter {
            credentials: self.credentials.clone(),
            sender: self.sender.clone(),
            templates: self.templates.clone(),
        }
    }
}

impl MoogleMailMaintenanceAdapter {
    async fn compose_session(
        &self,
        world: MoogleMailWorld,
    ) -> Result<MoogleMailCredentialSession, ApplicationError> {
        self.credentials.begin_exclusive(world, MoogleMailOperation::Compose).await.map_err(
            |diagnostic_cause| {
                safe_diagnostics::warn(
                    "mooglemail_maintenance",
                    "acquire_compose_session",
                    safe_diagnostics::project(&diagnostic_cause),
                    DiagnosticContext {
                        world: diagnostic_world(world.as_str()),
                        ..Default::default()
                    },
                );
                maintenance_error("MoogleMail 维护会话不可用")
            },
        )
    }

    async fn read_session(
        &self,
        world: MoogleMailWorld,
        operation: MoogleMailOperation,
    ) -> Result<MoogleMailCredentialSession, ApplicationError> {
        self.credentials.begin_read(world, operation).await.map_err(|diagnostic_cause| {
            safe_diagnostics::warn(
                "mooglemail_maintenance",
                "acquire_read_session",
                safe_diagnostics::project(&diagnostic_cause),
                DiagnosticContext { world: diagnostic_world(world.as_str()), ..Default::default() },
            );
            maintenance_error("MoogleMail 维护读取会话不可用")
        })
    }
}

impl MoogleMailboxMaintenance for MoogleMailMaintenanceAdapter {
    fn compose<'a>(&'a self, world_id: &'a str) -> MailboxFuture<'a, MoogleMailboxComposeState> {
        Box::pin(async move {
            let world = maintenance_world(world_id)?;
            let mut session = self.compose_session(world).await?;
            match self.sender.inspect_compose(session.cookie(), world).await {
                Ok(state) => {
                    check_observation_session(&session).await?;
                    Ok(MoogleMailboxComposeState {
                        clean: state.is_clean(),
                        body_digest: canonical_mail_body_digest(&state.body),
                        recipient: state.recipient,
                        subject: state.subject,
                        body: state.body,
                        credits: state.credits,
                        cod: state.cod,
                        attachment_count: state.attachment_count,
                        equipment_ids: state.equipment_ids,
                        observation_credential: session.observation_credential_binding(),
                    })
                }
                Err(error) => {
                    if error.is_session_expired() {
                        let _ = session.invalidate_current().await;
                    }
                    Err(maintenance_remote_error(error, "compose 读取失败"))
                }
            }
        })
    }

    fn discard_compose<'a>(
        &'a self,
        world_id: &'a str,
        expected: &'a MoogleMailboxComposeState,
    ) -> MailboxFuture<'a, MoogleMailboxDiscard> {
        Box::pin(async move {
            let world = maintenance_world(world_id)?;
            let mut session = self.compose_session(world).await?;
            check_observation_session(&session).await?;
            if session.observation_credential_binding() != expected.observation_credential {
                return Err(maintenance_error("compose 草稿观察的凭据或运行时已变化"));
            }
            match self.sender.discard_compose(session.cookie(), world, expected).await {
                Ok(MoogleMailDiscardOutcome::AlreadyClean) => {
                    Ok(MoogleMailboxDiscard::AlreadyClean)
                }
                Ok(MoogleMailDiscardOutcome::Discarded) => Ok(MoogleMailboxDiscard::Discarded),
                Err(error) => {
                    if error.category == MoogleMailErrorCategory::SessionExpired {
                        let _ = session.invalidate_current().await;
                    }
                    Err(maintenance_remote_error(error, "compose 草稿丢弃失败"))
                }
            }
        })
    }

    fn sent_entries<'a>(
        &'a self,
        world_id: &'a str,
    ) -> MailboxFuture<'a, MoogleMailboxSentEntries> {
        Box::pin(async move {
            let world = maintenance_world(world_id)?;
            let mut session = self.read_session(world, MoogleMailOperation::Sent).await?;
            match self.sender.scan_sent_summaries(session.cookie(), world).await {
                Ok(scan) => {
                    check_observation_session(&session).await?;
                    Ok(MoogleMailboxSentEntries {
                        entries: scan
                            .summaries
                            .into_iter()
                            .map(|entry| MoogleMailboxSentEntry {
                                external_mail_id: entry.external_mail_id,
                                recipient: entry.recipient,
                                subject: entry.subject,
                                sent_at: entry.sent_at,
                            })
                            .collect(),
                        covered_from: scan.covered_from,
                        complete: scan.complete,
                        page_observation: scan.page_observation,
                    })
                }
                Err(error) => {
                    if error.is_session_expired() {
                        let _ = session.invalidate_current().await;
                    }
                    Err(maintenance_remote_error(error, "Sent 列表读取失败"))
                }
            }
        })
    }

    fn read_mail_observed<'a>(&'a self, world_id: &'a str) -> MailboxFuture<'a, usize> {
        Box::pin(async move {
            let world = maintenance_world(world_id)?;
            let mut session = self.read_session(world, MoogleMailOperation::ReadMail).await?;
            match self.sender.scan_read_summaries(session.cookie(), world).await {
                Ok(entries) => {
                    check_observation_session(&session).await?;
                    // 历史 Read Mail 只产生计数，不回灌 inbox journal 或业务命令。
                    Ok(entries.len())
                }
                Err(error) => {
                    if error.is_session_expired() {
                        let _ = session.invalidate_current().await;
                    }
                    Err(maintenance_remote_error(error, "Read Mail 列表读取失败"))
                }
            }
        })
    }

    fn notice_expectation<'a>(
        &'a self,
        candidate: &'a MoogleMailboxUnprovenSend,
    ) -> MailboxFuture<'a, Option<MoogleMailboxNoticeExpectation>> {
        self.notice_expectation_inner(candidate)
    }

    fn inbox_assets<'a>(
        &'a self,
        world_id: &'a str,
        external_mail_id: &'a str,
    ) -> MailboxFuture<'a, MoogleMailboxInboxObservation> {
        Box::pin(async move {
            let world = maintenance_world(world_id)?;
            let mut session = self.read_session(world, MoogleMailOperation::Detail).await?;
            match self
                .sender
                .inspect_inbox_attachments(session.cookie(), world, external_mail_id)
                .await
            {
                Ok(assets) => {
                    check_observation_session(&session).await?;
                    Ok(MoogleMailboxInboxObservation {
                        assets: assets.map(|assets| MoogleMailboxInboxAssets {
                            sender: assets.sender,
                            equipment_ids: assets.equipment_ids,
                            credits: assets.credits,
                            cod: assets.cod,
                        }),
                        // 明确缺失也必须携带原观察会话身份，不能把换号后的缺席当作 Take 证明。
                        observation_credential: session.observation_credential_binding(),
                    })
                }
                Err(error) => {
                    if error.is_session_expired() {
                        let _ = session.invalidate_current().await;
                    }
                    Err(maintenance_remote_error(error, "收件箱详情读取失败"))
                }
            }
        })
    }

    fn sent_state<'a>(
        &'a self,
        world_id: &'a str,
        entry: &'a MoogleMailboxSentEntry,
        observed_at: DateTime<Utc>,
    ) -> MailboxFuture<'a, MoogleMailboxSentState> {
        Box::pin(async move {
            let world = maintenance_world(world_id)?;
            let mut session = self.read_session(world, MoogleMailOperation::SentDetail).await?;
            let summary = MoogleMailSentListEntry {
                external_mail_id: entry.external_mail_id.clone(),
                recipient: entry.recipient.clone(),
                subject: entry.subject.clone(),
                sent_at: entry.sent_at,
            };
            match self.sender.inspect_sent(session.cookie(), world, &summary, observed_at).await {
                Ok(state) => {
                    check_observation_session(&session).await?;
                    Ok(MoogleMailboxSentState {
                        external_mail_id: state.external_mail_id,
                        recipient: state.recipient,
                        subject: state.subject,
                        collected: state.collected,
                        cod_pending: state.cod_pending,
                        remaining_credits: state.remaining_credits,
                        remaining_equipment_ids: state.remaining_equipment_ids,
                        body_digest: state.body_digest,
                        observation_credential: session.observation_credential_binding(),
                    })
                }
                Err(error) => {
                    if error.is_session_expired() {
                        let _ = session.invalidate_current().await;
                    }
                    Err(maintenance_remote_error(error, "Sent 详情读取失败"))
                }
            }
        })
    }
}

impl MoogleMailMaintenanceAdapter {
    /// 旧通知仅保留无 key 且带原 MID 的保守证明；当前 key 无法证明历史实际发送字节。
    fn notice_expectation_inner<'a>(
        &'a self,
        candidate: &'a MoogleMailboxUnprovenSend,
    ) -> MailboxFuture<'a, Option<MoogleMailboxNoticeExpectation>> {
        Box::pin(async move {
            if candidate.send_expectation.is_some() || candidate.user_key_user_id.is_some() {
                return Ok(None);
            }
            let Some(result_code) = candidate.result_code.as_deref() else { return Ok(None) };
            // operations_alert 与 lot_watch 的旧正文未在此保存，不能用新模板猜测历史内容。
            if matches!(candidate.template.as_str(), "operations_alert" | "lot_watch") {
                return Ok(None);
            }
            let Some((subject, body)) = render_notification(
                &self.templates,
                &candidate.template,
                result_code,
                None,
                &candidate.recipient,
                &candidate.world_id,
                &candidate.inbound_mail_id,
            ) else {
                return Ok(None);
            };
            if subject.is_empty() || subject.len() > 512 || body.is_empty() {
                return Ok(None);
            }
            // 相同正文不能区分重复通知；只有实际保留入站 MID 的模板才可用于未知发送核对。
            let alternative_reference = format!("{}-reconcile-proof", candidate.inbound_mail_id);
            let Some((_, alternative_body)) = render_notification(
                &self.templates,
                &candidate.template,
                result_code,
                None,
                &candidate.recipient,
                &candidate.world_id,
                &alternative_reference,
            ) else {
                return Ok(None);
            };
            let body_digest = canonical_mail_body_digest(&body);
            if body_digest == canonical_mail_body_digest(&alternative_body) {
                return Ok(None);
            }
            Ok(Some(MoogleMailboxNoticeExpectation { subject, body_digest }))
        })
    }
}

async fn check_observation_session(
    session: &MoogleMailCredentialSession,
) -> Result<(), ApplicationError> {
    session.check_current().await.map_err(|diagnostic_cause| {
        safe_diagnostics::warn(
            "mooglemail_maintenance",
            "check_read_session",
            safe_diagnostics::project(&diagnostic_cause),
            DiagnosticContext::default(),
        );
        maintenance_error("MoogleMail 维护读取会话已失效")
    })
}

fn maintenance_world(world_id: &str) -> Result<MoogleMailWorld, ApplicationError> {
    MoogleMailWorld::parse(world_id).ok_or_else(|| maintenance_error("MoogleMail 维护世界无效"))
}

/// 远端读取失败保留 MoogleMail 分类（账号忙按正常退避降级，协议漂移等仍可见），
/// 不诊断为本地仓储故障；说明文本即维护报告的状态文案。
fn maintenance_remote_error(error: MoogleMailError, detail: &str) -> ApplicationError {
    ApplicationError::RemoteDependency {
        kind: error.safe_kind(),
        detail: format!(
            "{detail}: operation={} category={} write_started={}",
            error.operation.as_str(),
            error.category.as_str(),
            error.write_started()
        ),
    }
}

fn maintenance_error(message: &str) -> ApplicationError {
    RepositoryError::Failed(message.to_owned()).into()
}
