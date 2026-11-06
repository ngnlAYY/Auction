//! Admin 业务世界启停用例。

use std::sync::Arc;

use crate::errors::ApplicationError;
use crate::errors::diagnostics::{
    DiagnosticContext, DiagnosticError, DiagnosticLevel, record_error,
};
use crate::ports::admin::{
    ADMIN_WORLD_SET_ENABLED_COMMAND_KIND, AdminWorldError, AdminWorldPort,
    AdminWorldSetEnabledCommand, AdminWorldTransaction, admin_world_aggregate_id,
};
use crate::ports::auction::{ApplicationFuture, WorldReader, WorldSnapshot};
use crate::ports::core::{
    CompetitionTransaction, StoredCommand, TransactionPolicy, TransactionRunner,
};

/// 将业务世界状态、幂等账本和追加式审计收口到同一事务的应用服务。
pub struct AdminWorldService<R> {
    repository: Arc<R>,
}

impl<R> AdminWorldService<R> {
    pub fn new(repository: Arc<R>) -> Self {
        Self { repository }
    }
}

impl<R> AdminWorldService<R>
where
    R: WorldReader,
{
    pub async fn list_worlds(&self) -> Result<Vec<WorldSnapshot>, ApplicationError> {
        let worlds = self.repository.list_worlds().await?;
        if worlds.iter().any(|world| !world.validate()) {
            return Err(AdminWorldError::StoredResultCorrupt.into());
        }
        Ok(worlds)
    }
}

impl<R> AdminWorldService<R>
where
    R: TransactionRunner,
    R::Transaction: CompetitionTransaction + AdminWorldTransaction,
{
    /// 串行化 world 行、blocker 检查、版本 CAS、命令账本与审计。
    pub async fn set_enabled(
        &self,
        command: AdminWorldSetEnabledCommand,
    ) -> Result<WorldSnapshot, ApplicationError> {
        command.validate()?;
        let request_hash = command.request_hash()?;
        let aggregate_id = admin_world_aggregate_id(&command.world_id);

        self.repository
            .run(TransactionPolicy::serializable(), |transaction| {
                let command = command.clone();
                Box::pin(async move {
                    if let Some(stored) = transaction.find_command(command.command_id).await? {
                        return replay(stored, &command, aggregate_id, request_hash);
                    }

                    let previous = transaction
                        .load_world_for_update(&command.world_id)
                        .await?
                        .ok_or(AdminWorldError::NotFound)?;
                    if !previous.validate() {
                        return Err(AdminWorldError::StoredResultCorrupt.into());
                    }
                    if previous.policy_revision != command.expected_policy_revision {
                        return Err(AdminWorldError::PolicyRevisionConflict {
                            current_revision: previous.policy_revision,
                        }
                        .into());
                    }

                    let changed = previous.enabled != command.enabled;
                    if changed
                        && !command.enabled
                        && transaction.has_world_deactivation_blocker(&command.world_id).await?
                    {
                        return Err(AdminWorldError::ActiveAuction.into());
                    }

                    let result = if changed {
                        transaction
                            .update_world_enabled(
                                &command.world_id,
                                command.enabled,
                                command.expected_policy_revision,
                            )
                            .await?
                    } else {
                        previous.clone()
                    };
                    if !valid_result(&command, &result, changed) {
                        return Err(AdminWorldError::StoredResultCorrupt.into());
                    }

                    transaction
                        .insert_command(StoredCommand {
                            command_id: command.command_id,
                            command_kind: ADMIN_WORLD_SET_ENABLED_COMMAND_KIND.to_owned(),
                            aggregate_id,
                            request_hash,
                            result_code: "accepted".to_owned(),
                            result_payload: serde_json::to_vec(&result)?,
                        })
                        .await?;
                    transaction
                        .append_admin_world_record(command, previous, result.clone(), changed)
                        .await?;
                    Ok(result)
                })
            })
            .await
    }
}

impl<R> AdminWorldPort for AdminWorldService<R>
where
    R: TransactionRunner + WorldReader + 'static,
    R::Transaction: CompetitionTransaction + AdminWorldTransaction,
{
    fn list_worlds(&self) -> ApplicationFuture<'_, Vec<WorldSnapshot>> {
        Box::pin(async move { AdminWorldService::list_worlds(self).await })
    }

    fn set_enabled(
        &self,
        command: AdminWorldSetEnabledCommand,
    ) -> ApplicationFuture<'_, WorldSnapshot> {
        Box::pin(async move { AdminWorldService::set_enabled(self, command).await })
    }
}

fn replay(
    stored: StoredCommand,
    command: &AdminWorldSetEnabledCommand,
    aggregate_id: uuid::Uuid,
    request_hash: crate::ports::core::RequestHash,
) -> Result<WorldSnapshot, ApplicationError> {
    if !stored.matches_request(ADMIN_WORLD_SET_ENABLED_COMMAND_KIND, aggregate_id, request_hash) {
        return Err(AdminWorldError::CommandConflict.into());
    }
    if stored.result_code != "accepted" {
        return Err(AdminWorldError::StoredResultCorrupt.into());
    }
    let result: WorldSnapshot =
        serde_json::from_slice(&stored.result_payload).map_err(|error| {
            record_error(
                DiagnosticLevel::Warn,
                error.diagnostic_kind(),
                "admin_worlds",
                "decode_replay_result",
                DiagnosticContext::default(),
            );
            AdminWorldError::StoredResultCorrupt
        })?;
    if !valid_replay(command, &result) {
        return Err(AdminWorldError::StoredResultCorrupt.into());
    }
    Ok(result)
}

fn valid_result(
    command: &AdminWorldSetEnabledCommand,
    result: &WorldSnapshot,
    changed: bool,
) -> bool {
    let expected_revision = if changed {
        command.expected_policy_revision.checked_add(1)
    } else {
        Some(command.expected_policy_revision)
    };
    result.validate()
        && result.id == command.world_id
        && result.enabled == command.enabled
        && Some(result.policy_revision) == expected_revision
}

fn valid_replay(command: &AdminWorldSetEnabledCommand, result: &WorldSnapshot) -> bool {
    let next_revision = command.expected_policy_revision.checked_add(1);
    result.validate()
        && result.id == command.world_id
        && result.enabled == command.enabled
        && (result.policy_revision == command.expected_policy_revision
            || Some(result.policy_revision) == next_revision)
}
