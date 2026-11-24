//! Admin 装备来源录入用例。
//! 外部读取在数据库事务之前完成，写事务中再次检查命令账本；来源失败不产生半成品 lot，
//! 重复命令直接重放已持久化的安全结果。

use std::sync::Arc;

use auction_domain::consignment::ListingEquipmentIdentity;
use auction_domain::{CommandId, EquipmentId};
use uuid::Uuid;

use crate::errors::ApplicationError;
use crate::errors::diagnostics::{
    DiagnosticContext, DiagnosticError, DiagnosticLevel, record_error,
};
use crate::ports::admin::{
    AdminIntakeCommand, AdminIntakeError, AdminIntakePolicy, AdminIntakePort, AdminIntakeRecord,
    AdminIntakeResult, AdminIntakeSourceKind, AdminIntakeSourceReader, AdminIntakeTransaction,
    AdminIntakeWrite, EquipmentKeySealerPort, admin_intake_aggregate_id,
};
use crate::ports::core::ApplicationFuture;
use crate::ports::core::competition::canonical_equipment_lock_identity;
use crate::ports::core::{
    Clock, CompetitionTransaction, CompetitionTransactionError, StoredCommand, TransactionPolicy,
    TransactionRunner,
};

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
        // 来源采集时间保持原文；读取完成后再校验，聚合写入时间由持锁事务单独取得。
        snapshot.validate_for_admin(self.clock.now(), self.policy)?;

        let equipment_id = EquipmentId::from_uuid(Uuid::new_v4());
        let submission_id = Uuid::new_v4();
        let lot_id = Uuid::new_v4();
        let source_reference = format!("{}:{}", snapshot.adapter, snapshot.external_equipment_id);
        let key_envelope = self.sealer.seal(equipment_id, &snapshot.key)?;
        // 不变量 2：Admin 建 lot 与寄售强占用共用同一实物锁；原文 EID 先按数据库别名规则
        // 规范化，无法构造身份的旧值不取锁并保持原有录入行为。
        let equipment_identities =
            canonical_equipment_lock_identity(&command.world_id, &snapshot.external_equipment_id)
                .into_iter()
                .collect::<Vec<_>>();
        // 维护者 2026-11-23 决定：只有该实物此刻已有寄售预约或 position 持有才取实物租约。
        // 无持有时保持既有 `run` 路径；预判之后才签发的预约由事务内强占用检查、Serializable
        // 与 lot 守卫兜底。无法构造规范身份的件不查询、不取锁。
        let equipment_held = if equipment_identities.is_empty() {
            false
        } else {
            self.equipment_held(&equipment_identities).await?
        };
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
            external_equipment_id: snapshot.external_equipment_id,
            equipment_name: snapshot.name,
            equipment_category: snapshot.category,
            equipment_level: snapshot.level,
            equipment_attributes: snapshot.attributes,
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
            .run_with_equipment_gate(
                equipment_held,
                &equipment_identities,
                TransactionPolicy::serializable(),
                move |transaction| {
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
                        result.validate().map_err(|error| {
                            record_error(
                                DiagnosticLevel::Warn,
                                error.diagnostic_kind(),
                                "admin_intake",
                                "validate_owner_result",
                                DiagnosticContext::default(),
                            );
                            AdminIntakeError::StoredResultCorrupt
                        })?;
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
                },
            )
            .await;
        // 既有 Admin 录入用“装备已存在”统一表达实物冲突；寄售强占用不新增 HTTP 错误码或译文，
        // 因此在这里映射到同一个既有拒绝，由 HTTP 边界保持 409 equipment_conflict。
        let result = match result {
            Err(ApplicationError::CompetitionTransaction(
                CompetitionTransactionError::EquipmentHeld,
            )) => Err(CompetitionTransactionError::AdminIntakeEquipmentConflict.into()),
            other => other,
        }?;
        Ok(result)
    }

    async fn equipment_held(
        &self,
        equipment: &[ListingEquipmentIdentity],
    ) -> Result<bool, ApplicationError> {
        let equipment = equipment.to_vec();
        self.repository
            .run(TransactionPolicy::read_committed(), move |transaction| {
                let equipment = equipment.clone();
                Box::pin(async move {
                    transaction.any_equipment_held(&equipment).await.map_err(ApplicationError::from)
                })
            })
            .await
    }

    async fn find_replay(
        &self,
        command_id: CommandId,
        aggregate_id: Uuid,
        request_hash: crate::ports::core::RequestHash,
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
    request_hash: crate::ports::core::RequestHash,
) -> Result<AdminIntakeResult, ApplicationError> {
    if !stored.matches_request(command_kind, aggregate_id, request_hash) {
        return Err(AdminIntakeError::CommandConflict.into());
    }
    if stored.result_code != "accepted" {
        return Err(AdminIntakeError::StoredResultCorrupt.into());
    }
    let result: AdminIntakeResult =
        serde_json::from_slice(&stored.result_payload).map_err(|error| {
            record_error(
                DiagnosticLevel::Warn,
                error.diagnostic_kind(),
                "admin_intake",
                "decode_replay_result",
                DiagnosticContext::default(),
            );
            AdminIntakeError::StoredResultCorrupt
        })?;
    result.validate().map_err(|error| {
        record_error(
            DiagnosticLevel::Warn,
            error.diagnostic_kind(),
            "admin_intake",
            "validate_replay_result",
            DiagnosticContext::default(),
        );
        AdminIntakeError::StoredResultCorrupt
    })?;
    Ok(result)
}
