//! Admin 站点文档（规则、关于）发布用例。

use std::sync::Arc;

use crate::errors::ApplicationError;
use crate::ports::admin::{
    AdminRulesDocument, AdminRulesError, AdminRulesPublishCommand, AdminRulesStoredCommand,
    AdminRulesTransaction, AdminRulesTransactionRunner,
};
use crate::ports::core::{Clock, TransactionPolicy};

const COMMAND_KIND: &str = "publish";

/// Admin 规则发布用例。
pub struct AdminRulesService<R, C> {
    repository: Arc<R>,
    clock: Arc<C>,
}

impl<R, C> AdminRulesService<R, C>
where
    R: AdminRulesTransactionRunner,
    C: Clock,
{
    /// 建立只依赖规则事务端口和时钟的发布用例。
    pub fn new(repository: Arc<R>, clock: Arc<C>) -> Self {
        Self { repository, clock }
    }

    /// 在同一 Serializable 事务中完成版本 CAS、命令账本和追加式审计。
    pub async fn publish(
        &self,
        command: AdminRulesPublishCommand,
    ) -> Result<AdminRulesDocument, ApplicationError> {
        let command = command.normalize();
        command.validate().map_err(ApplicationError::from)?;
        let expected_revision =
            command.expected_revision.checked_add(1).ok_or(AdminRulesError::InvalidCommand)?;
        let published_at = self.clock.now();
        self.repository
            .run(TransactionPolicy::serializable(), |transaction| {
                let command = command.clone();
                Box::pin(async move {
                    if let Some(stored) = transaction.find_command(command.command_id).await? {
                        if !stored.matches_request(
                            COMMAND_KIND,
                            command.kind,
                            &command.locale,
                            command.request_hash,
                        ) {
                            return Err(AdminRulesError::CommandConflict.into());
                        }
                        stored.validate().map_err(ApplicationError::from)?;
                        let result: AdminRulesDocument =
                            serde_json::from_slice(&stored.result_payload)?;
                        if result.validate().is_err()
                            || result.locale != command.locale
                            || result.revision != expected_revision
                        {
                            return Err(AdminRulesError::StoredResultCorrupt.into());
                        }
                        return Ok(result);
                    }

                    let result =
                        transaction.publish_document(command.clone(), published_at).await?;
                    if result.validate().is_err()
                        || result.locale != command.locale
                        || result.published_by != command.actor_id
                        || result.revision != expected_revision
                    {
                        return Err(AdminRulesError::StoredResultCorrupt.into());
                    }
                    transaction
                        .insert_command(AdminRulesStoredCommand {
                            command_id: command.command_id,
                            command_kind: COMMAND_KIND.to_owned(),
                            kind: command.kind,
                            locale: command.locale.clone(),
                            request_hash: command.request_hash,
                            result_payload: serde_json::to_vec(&result)?,
                        })
                        .await?;
                    transaction.append_publish_record(command, result.clone()).await?;
                    Ok(result)
                })
            })
            .await
    }
}
