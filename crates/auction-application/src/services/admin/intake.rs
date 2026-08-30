//! Admin 装备来源录入用例。
//!
//! 该服务明确把外部读取放在数据库事务之前，并在写事务中再次检查命令账本；来源失败不会
//! 产生半成品 lot，重复命令则直接重放已持久化的安全结果。

use std::sync::Arc;

use auction_domain::{CommandId, EquipmentId};
use uuid::Uuid;

use crate::{
    AdminIntakeCommand, AdminIntakeError, AdminIntakePolicy, AdminIntakePort, AdminIntakeRecord,
    AdminIntakeResult, AdminIntakeSourceKind, AdminIntakeSourceReader, AdminIntakeTransaction,
    AdminIntakeWrite, ApplicationError, ApplicationFuture, Clock, CompetitionTransaction,
    EquipmentKeySealerPort, StoredCommand, TransactionPolicy, TransactionRunner,
    admin_intake_aggregate_id,
};

/// Admin 装备录入应用服务。
pub struct AdminIntakeService<R, S, K, C> {
    repository: Arc<R>,
    source: Arc<S>,
    sealer: Arc<K>,
    clock: Arc<C>,
    policy: AdminIntakePolicy,
}

impl<R, S, K, C> AdminIntakeService<R, S, K, C>
where
    R: TransactionRunner,
    R::Transaction: CompetitionTransaction + AdminIntakeTransaction,
    S: AdminIntakeSourceReader,
    K: EquipmentKeySealerPort,
    C: Clock,
{
    /// 建立只依赖端口的装备录入用例。
    pub fn new(
        repository: Arc<R>,
        source: Arc<S>,
        sealer: Arc<K>,
        clock: Arc<C>,
        policy: AdminIntakePolicy,
    ) -> Self {
        Self { repository, source, sealer, clock, policy }
    }

    /// 执行一次 world-scoped 装备录入。
    pub async fn add_item(
        &self,
        mut command: AdminIntakeCommand,
    ) -> Result<AdminIntakeResult, ApplicationError> {
        // 旧版在校验和请求摘要之前规范化玩家 ID；重试时首尾空白不能形成另一条幂等命令。
        command.player_id = command.player_id.trim().to_owned();
        command.validate()?;
        let aggregate_id = admin_intake_aggregate_id(&command.world_id);
        let request_hash = command.request_hash(aggregate_id)?;

        if let Some(result) =
            self.find_replay(command.command_id, aggregate_id, request_hash).await?
        {
            return Ok(result);
        }

        let now = self.clock.now();
        let source_kind = command.normalized_source_kind();
        let mut snapshot = match source_kind {
            AdminIntakeSourceKind::Url => {
                self.source
                    .read_url(
                        command.world_id.clone(),
                        command.equipment_url.clone().ok_or(AdminIntakeError::InvalidCommand)?,
                    )
                    .await?
            }
            AdminIntakeSourceKind::Json => {
                self.source
                    .read_json(
                        command.world_id.clone(),
                        command.equipment.take().ok_or(AdminIntakeError::InvalidCommand)?,
                    )
                    .await?
            }
        };
        snapshot.collected_at = now;
        snapshot.validate_for_admin(now, self.policy)?;

        let equipment_id = EquipmentId::from_uuid(Uuid::new_v4());
        let submission_id = Uuid::new_v4();
        let lot_id = Uuid::new_v4();
        let source_reference = format!("{}:{}", snapshot.adapter, snapshot.external_equipment_id);
        let key_envelope = self.sealer.seal(equipment_id, &snapshot.key)?;
        snapshot.clear_secret();
        let write = AdminIntakeWrite {
            command_id: command.command_id,
            aggregate_id,
            world_id: command.world_id.clone(),
            source_kind,
            source_reference,
            source_fingerprint: snapshot.source_fingerprint,
            actor_id: command.actor_id.clone(),
            request_id: command.request_id.clone(),
            player_id: command.player_id.clone(),
            start_price: command.start_price,
            candidate_equipment_id: equipment_id,
            candidate_submission_id: submission_id,
            candidate_lot_id: lot_id,
            external_equipment_id: snapshot.external_equipment_id.clone(),
            equipment_name: snapshot.name.clone(),
            equipment_category: snapshot.category.clone(),
            equipment_level: snapshot.level,
            equipment_attributes: snapshot.attributes.clone(),
            collected_at: snapshot.collected_at,
            key_envelope,
            request_hash,
        };
        write.validate()?;

        let command_id = command.command_id;
        let actor_id = command.actor_id;
        let request_id = command.request_id;
        let world_id = command.world_id;
        let result = self
            .repository
            .run(TransactionPolicy::serializable(), move |transaction| {
                let write = write.clone();
                let actor_id = actor_id.clone();
                let request_id = request_id.clone();
                let world_id = world_id.clone();
                Box::pin(async move {
                    if let Some(stored) = transaction.find_command(command_id).await? {
                        return replay_result(
                            stored,
                            "admin_auction_intake_item",
                            aggregate_id,
                            request_hash,
                        );
                    }

                    let result = transaction.add_admin_intake(write).await?;
                    result.validate().map_err(|_| AdminIntakeError::StoredResultCorrupt)?;
                    transaction
                        .append_admin_intake_record(AdminIntakeRecord {
                            command_id,
                            world_id,
                            actor_id,
                            request_id,
                            result: result.clone(),
                        })
                        .await?;
                    Ok(result)
                })
            })
            .await?;
        Ok(result)
    }

    async fn find_replay(
        &self,
        command_id: CommandId,
        aggregate_id: Uuid,
        request_hash: crate::RequestHash,
    ) -> Result<Option<AdminIntakeResult>, ApplicationError> {
        let stored = self
            .repository
            .run(TransactionPolicy::read_committed(), move |transaction| {
                Box::pin(async move {
                    transaction.find_command(command_id).await.map_err(ApplicationError::from)
                })
            })
            .await?;
        stored
            .map(|value| {
                replay_result(value, "admin_auction_intake_item", aggregate_id, request_hash)
            })
            .transpose()
    }
}

impl<R, S, K, C> AdminIntakePort for AdminIntakeService<R, S, K, C>
where
    R: TransactionRunner + 'static,
    R::Transaction: CompetitionTransaction + AdminIntakeTransaction,
    S: AdminIntakeSourceReader + 'static,
    K: EquipmentKeySealerPort + 'static,
    C: Clock + 'static,
{
    fn add_item(&self, command: AdminIntakeCommand) -> ApplicationFuture<'_, AdminIntakeResult> {
        Box::pin(async move { AdminIntakeService::add_item(self, command).await })
    }
}

fn replay_result(
    stored: StoredCommand,
    command_kind: &str,
    aggregate_id: Uuid,
    request_hash: crate::RequestHash,
) -> Result<AdminIntakeResult, ApplicationError> {
    if !stored.matches_request(command_kind, aggregate_id, request_hash) {
        return Err(AdminIntakeError::CommandConflict.into());
    }
    if stored.result_code != "accepted" {
        return Err(AdminIntakeError::StoredResultCorrupt.into());
    }
    let result: AdminIntakeResult = serde_json::from_slice(&stored.result_payload)
        .map_err(|_| AdminIntakeError::StoredResultCorrupt)?;
    result.validate().map_err(|_| AdminIntakeError::StoredResultCorrupt)?;
    Ok(result)
}
