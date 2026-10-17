//! Admin 邮件轮询暂停命令用例。
//! 读取路径由邮件 owner 直接提供安全投影；暂停/恢复属于可变命令，必须在 PostgreSQL
//! Serializable 事务中完成状态版本 CAS、命令账本、审计和可重放结果。

use std::sync::Arc;

use crate::{
    AdminMailError, AdminMailPollState, AdminMailPollTriggerCommand, AdminMailPollingCommand,
    AdminMailStoredCommand, AdminMailTransaction, AdminMailTransactionRunner, ApplicationError,
    Clock, TransactionPolicy,
};

const COMMAND_KIND: &str = "set_polling_paused";
const TRIGGER_COMMAND_KIND: &str = "trigger_poll";

pub struct AdminMailService<R, C> {
    repository: Arc<R>,
    clock: Arc<C>,
}

impl<R, C> AdminMailService<R, C>
where
    R: AdminMailTransactionRunner,
    C: Clock,
{
    /// 建立只依赖邮件事务端口和时钟的命令用例。
    pub fn new(repository: Arc<R>, clock: Arc<C>) -> Self {
        Self { repository, clock }
    }

    /// 在同一 Serializable 事务中完成轮询暂停状态的版本 fence 与幂等记录。
    pub async fn set_polling_paused(
        &self,
        command: AdminMailPollingCommand,
    ) -> Result<AdminMailPollState, ApplicationError> {
        command.validate().map_err(ApplicationError::from)?;
        let updated_at = self.clock.now();
        self.repository
            .run(TransactionPolicy::serializable(), |transaction| {
                let command = command.clone();
                Box::pin(async move {
                    if let Some(stored) = transaction.find_command(command.command_id).await? {
                        if !stored.matches_request(&command) {
                            return Err(AdminMailError::CommandConflict.into());
                        }
                        stored.validate().map_err(ApplicationError::from)?;
                        let result: AdminMailPollState =
                            serde_json::from_slice(&stored.result_payload)?;
                        let expected_version = command
                            .expected_state_version
                            .checked_add(1)
                            .ok_or(AdminMailError::InvalidCommand)?;
                        if !result.validate()
                            || result.mailbox_key != command.mailbox_key
                            || result.paused != command.paused
                            || result.state_version != expected_version
                        {
                            return Err(AdminMailError::StoredResultCorrupt.into());
                        }
                        return Ok(result);
                    }

                    let result =
                        transaction.set_polling_paused(command.clone(), updated_at).await?;
                    let expected_version = command
                        .expected_state_version
                        .checked_add(1)
                        .ok_or(AdminMailError::InvalidCommand)?;
                    if !result.validate()
                        || result.mailbox_key != command.mailbox_key
                        || result.paused != command.paused
                        || result.state_version != expected_version
                    {
                        return Err(AdminMailError::InvalidProjection.into());
                    }
                    transaction
                        .insert_command(AdminMailStoredCommand {
                            command_id: command.command_id,
                            command_kind: COMMAND_KIND.to_owned(),
                            mailbox_key: command.mailbox_key.clone(),
                            request_hash: command.request_hash,
                            result_payload: serde_json::to_vec(&result)?,
                        })
                        .await?;
                    transaction.append_pause_record(command, result.clone()).await?;
                    Ok(result)
                })
            })
            .await
    }

    /// 在同一 Serializable 事务中提前 next_poll_at，并保存可重放的触发结果。
    pub async fn trigger_poll(
        &self,
        command: AdminMailPollTriggerCommand,
    ) -> Result<AdminMailPollState, ApplicationError> {
        command.validate().map_err(ApplicationError::from)?;
        let triggered_at = self.clock.now();
        self.repository
            .run(TransactionPolicy::serializable(), |transaction| {
                let command = command.clone();
                Box::pin(async move {
                    if let Some(stored) = transaction.find_command(command.command_id).await? {
                        if !stored.matches_trigger_request(&command) {
                            return Err(AdminMailError::CommandConflict.into());
                        }
                        stored.validate().map_err(ApplicationError::from)?;
                        let result: AdminMailPollState =
                            serde_json::from_slice(&stored.result_payload)?;
                        let expected_version = command
                            .expected_state_version
                            .checked_add(1)
                            .ok_or(AdminMailError::InvalidCommand)?;
                        if !result.validate()
                            || result.mailbox_key != command.mailbox_key
                            || result.state_version != expected_version
                        {
                            return Err(AdminMailError::StoredResultCorrupt.into());
                        }
                        return Ok(result);
                    }

                    let result = transaction.trigger_poll(command.clone(), triggered_at).await?;
                    let expected_version = command
                        .expected_state_version
                        .checked_add(1)
                        .ok_or(AdminMailError::InvalidCommand)?;
                    if !result.validate()
                        || result.mailbox_key != command.mailbox_key
                        || result.state_version != expected_version
                    {
                        return Err(AdminMailError::InvalidProjection.into());
                    }
                    transaction
                        .insert_command(AdminMailStoredCommand {
                            command_id: command.command_id,
                            command_kind: TRIGGER_COMMAND_KIND.to_owned(),
                            mailbox_key: command.mailbox_key.clone(),
                            request_hash: command.request_hash,
                            result_payload: serde_json::to_vec(&result)?,
                        })
                        .await?;
                    transaction.append_trigger_record(command, result.clone()).await?;
                    Ok(result)
                })
            })
            .await
    }
}
