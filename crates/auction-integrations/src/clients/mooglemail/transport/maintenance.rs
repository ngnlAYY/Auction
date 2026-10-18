//! 定期维护适配：复用 delivery transport 的凭据集合与发送器读取三页。
//!
//! 适配器只持有可克隆的凭据集合与发送器，不接管 transport 的 delivery 状态；读取与
//! discard 都走同一排他凭据会话，避免与发送并发覆盖 compose。

use auction_application::{
    ApplicationError, MailboxFuture, MoogleMailboxComposeState, MoogleMailboxDiscard,
    MoogleMailboxMaintenance, MoogleMailboxSentEntry, MoogleMailboxSentState, RepositoryError,
};
use chrono::{DateTime, Utc};

use super::{
    super::{MoogleMailDiscardOutcome, MoogleMailSentListEntry},
    *,
};

/// 三页维护的远端适配器；不包含投递 claim、租约或重试状态。
pub struct MoogleMailMaintenanceAdapter {
    credentials: MoogleMailCredentialSet,
    sender: MoogleMailSender,
}

impl MoogleMailDeliveryTransport {
    /// 从当前 transport 复制凭据集合与发送器，供维护循环独立持有。
    pub fn maintenance_adapter(&self) -> MoogleMailMaintenanceAdapter {
        MoogleMailMaintenanceAdapter {
            credentials: self.credentials.clone(),
            sender: self.sender.clone(),
        }
    }
}

impl MoogleMailMaintenanceAdapter {
    async fn session(
        &self,
        world: MoogleMailWorld,
    ) -> Result<MoogleMailCredentialSession, ApplicationError> {
        self.credentials
            .begin_exclusive(world, MoogleMailOperation::Compose)
            .await
            .map_err(|_| maintenance_error("MoogleMail 维护会话不可用"))
    }
}

impl MoogleMailboxMaintenance for MoogleMailMaintenanceAdapter {
    fn compose<'a>(&'a self, world_id: &'a str) -> MailboxFuture<'a, MoogleMailboxComposeState> {
        Box::pin(async move {
            let world = maintenance_world(world_id)?;
            let mut session = self.session(world).await?;
            match self.sender.inspect_compose(session.cookie(), world).await {
                Ok(state) => Ok(MoogleMailboxComposeState {
                    clean: state.is_clean(),
                    recipient: state.recipient,
                    subject: state.subject,
                    body: state.body,
                    credits: state.credits,
                    cod: state.cod,
                    attachment_count: state.attachment_count,
                }),
                Err(error) => {
                    if error.is_session_expired() {
                        let _ = session.invalidate_current().await;
                    }
                    Err(maintenance_remote_error("compose 读取失败"))
                }
            }
        })
    }

    fn discard_compose<'a>(&'a self, world_id: &'a str) -> MailboxFuture<'a, MoogleMailboxDiscard> {
        Box::pin(async move {
            let world = maintenance_world(world_id)?;
            let mut session = self.session(world).await?;
            match self.sender.discard_compose(session.cookie(), world).await {
                Ok(MoogleMailDiscardOutcome::AlreadyClean) => {
                    Ok(MoogleMailboxDiscard::AlreadyClean)
                }
                Ok(MoogleMailDiscardOutcome::Discarded) => Ok(MoogleMailboxDiscard::Discarded),
                Err(error) => {
                    if error.is_session_expired() {
                        let _ = session.invalidate_current().await;
                    }
                    Err(maintenance_remote_error("compose 草稿丢弃失败"))
                }
            }
        })
    }

    fn sent_entries<'a>(
        &'a self,
        world_id: &'a str,
    ) -> MailboxFuture<'a, Vec<MoogleMailboxSentEntry>> {
        Box::pin(async move {
            let world = maintenance_world(world_id)?;
            let mut session = self.session(world).await?;
            match self.sender.scan_sent_summaries(session.cookie(), world).await {
                Ok(entries) => Ok(entries
                    .into_iter()
                    .map(|entry| MoogleMailboxSentEntry {
                        external_mail_id: entry.external_mail_id,
                        recipient: entry.recipient,
                        subject: entry.subject,
                        sent_at: entry.sent_at,
                    })
                    .collect()),
                Err(error) => {
                    if error.is_session_expired() {
                        let _ = session.invalidate_current().await;
                    }
                    Err(maintenance_remote_error("Sent 列表读取失败"))
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
            let mut session = self.session(world).await?;
            let summary = MoogleMailSentListEntry {
                external_mail_id: entry.external_mail_id.clone(),
                recipient: entry.recipient.clone(),
                subject: entry.subject.clone(),
                sent_at: entry.sent_at,
            };
            match self.sender.inspect_sent(session.cookie(), world, &summary, observed_at).await {
                Ok(state) => Ok(MoogleMailboxSentState {
                    external_mail_id: state.external_mail_id,
                    collected: state.collected,
                    cod_pending: state.cod_pending,
                    remaining_credits: state.remaining_credits,
                    remaining_equipment_ids: state.remaining_equipment_ids,
                }),
                Err(error) => {
                    if error.is_session_expired() {
                        let _ = session.invalidate_current().await;
                    }
                    Err(maintenance_remote_error("Sent 详情读取失败"))
                }
            }
        })
    }
}

fn maintenance_world(world_id: &str) -> Result<MoogleMailWorld, ApplicationError> {
    MoogleMailWorld::parse(world_id).ok_or_else(|| maintenance_error("MoogleMail 维护世界无效"))
}

fn maintenance_remote_error(detail: &str) -> ApplicationError {
    maintenance_error(detail)
}

fn maintenance_error(message: &str) -> ApplicationError {
    RepositoryError::Failed(message.to_owned()).into()
}
