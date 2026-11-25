//! 内部满足事务装配；资金 SQL 唯一拥有者仍为 settlement_funding。

use auction_application::core::ApplicationFuture;
use auction_application::settlement::FundingError;
use auction_application::settlement::{
    FundingFuture, SettlementFulfillmentProof, SettlementInternalCommand,
    SettlementInternalCommitOutcome, SettlementInternalEvidenceSnapshot,
    SettlementInternalFulfillmentStore,
};

use crate::database::settlement::funding::{evidence_error, storage as sql_error};
use crate::database::storage_error;

mod conflicts;
mod graph;
mod locks;
mod persistence;
mod transaction;
mod validation;

pub(crate) use conflicts::{assetless_mail_notification, exclude_equipment_loss_conflicts};

/// 集成测试入口：在回滚事务内对任意 EID 跑一次装备损失的资产冲突门，只返回是否放行；
/// 标的、付款与原投递取随机 ID，因此只有全库出站与历史投递会参与判定。
#[cfg(feature = "test-db")]
pub async fn asset_conflict_gate_passes(
    store: &crate::DatabaseStore,
    external_equipment_id: &str,
) -> Result<bool, FundingError> {
    let mut tx = store.pool().begin().await.map_err(sql_error)?;
    let outcome = exclude_equipment_loss_conflicts(
        &mut tx,
        uuid::Uuid::new_v4(),
        uuid::Uuid::new_v4(),
        external_equipment_id,
        uuid::Uuid::new_v4(),
    )
    .await;
    tx.rollback().await.map_err(sql_error)?;
    match outcome {
        Ok(()) => Ok(true),
        Err(FundingError::Unproven) => Ok(false),
        Err(error) => Err(error),
    }
}

impl SettlementInternalFulfillmentStore for crate::DatabaseStore {
    fn legacy_self_reconciliation_uid(
        &self,
        obligation_id: uuid::Uuid,
    ) -> ApplicationFuture<'_, Option<String>> {
        Box::pin(async move {
            let mut tx = self.pool().begin().await.map_err(|error| {
                crate::database::log_discarded_sql_error(
                    &error,
                    "legacy_self_proof_read_failed",
                    "开启旧自寄修复证明事务失败",
                );
                storage_error("旧自寄修复证明不可用")
            })?;
            let result = crate::database::delivery::legacy_self_proof::legacy_self_uid(
                &mut tx,
                obligation_id,
            )
            .await
            .map_err(|_| storage_error("旧自寄修复证明不匹配"))?;
            tx.commit().await.map_err(|error| {
                crate::database::log_discarded_sql_error(
                    &error,
                    "legacy_self_proof_read_failed",
                    "提交旧自寄修复证明读取事务失败",
                );
                storage_error("旧自寄修复证明读取失败")
            })?;
            Ok(result)
        })
    }
    fn replay_internal(&self, command: SettlementInternalCommand) -> ApplicationFuture<'_, bool> {
        Box::pin(async move {
            persistence::replay_pool(self.pool(), &command).await.map_err(|error| match error {
                FundingError::Conflict => {
                    auction_application::core::ApplicationError::FulfillmentDependencyMismatch
                }
                _ => storage_error("内部命令账本暂不可用"),
            })
        })
    }

    fn satisfy_internal<'a>(
        &'a self,
        command: &'a SettlementInternalCommand,
        evidence: &'a SettlementInternalEvidenceSnapshot,
    ) -> FundingFuture<'a, SettlementInternalCommitOutcome> {
        Box::pin(async move { transaction::run(self.pool(), command, evidence, None, false).await })
    }

    fn prepare_external_refund<'a>(
        &'a self,
        command: &'a SettlementInternalCommand,
        evidence: &'a SettlementInternalEvidenceSnapshot,
    ) -> FundingFuture<'a, SettlementInternalCommitOutcome> {
        Box::pin(async move { transaction::run(self.pool(), command, evidence, None, true).await })
    }

    fn satisfy_collected<'a>(
        &'a self,
        command: &'a SettlementInternalCommand,
        proof: &'a SettlementFulfillmentProof,
        evidence: &'a SettlementInternalEvidenceSnapshot,
    ) -> FundingFuture<'a, SettlementInternalCommitOutcome> {
        Box::pin(async move {
            transaction::run(self.pool(), command, evidence, Some(proof), false).await
        })
    }
}

/// 通用内部执行器不是寄售义务的 owner：寄售义务只经寄售派发轮，显式失败关闭，保护不变量 6。
/// 寄售的收件人授权在派发轮起写处由 `authorize_entitlement_recipient_in_tx` 完成。
///
/// 只读探针、不加锁，须先于任何拍卖锁；拍卖义务与不存在的义务 ID 放行，后续校验照旧判定。
async fn reject_consignment_obligation(
    tx: &mut crate::database::TrackedTransaction<'_, sqlx::Postgres>,
    obligation_id: uuid::Uuid,
) -> Result<(), FundingError> {
    let consignment: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM public.settlement_obligations
                       WHERE id=$1 AND contract_id IS NOT NULL)",
    )
    .bind(obligation_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(sql_error)?;
    if consignment {
        return Err(FundingError::Unproven);
    }
    Ok(())
}

/// 旧prepare/activate/satisfy也遵守同一业务锁序，不能在fulfillment后反锁义务。
pub(super) async fn lock_legacy(
    tx: &mut crate::database::TrackedTransaction<'_, sqlx::Postgres>,
    claim: &auction_application::settlement::SettlementFulfillmentClaim,
    custody: Option<(&SettlementInternalEvidenceSnapshot, uuid::Uuid)>,
) -> Result<bool, FundingError> {
    reject_consignment_obligation(tx, claim.obligation_id.as_uuid()).await?;
    if claim.kind == auction_domain::SettlementObligationKind::BuyerRefund
        && claim.state != auction_application::settlement::SettlementFulfillmentState::Enqueued
    {
        let (evidence, operation_id) = custody.ok_or(FundingError::Unproven)?;
        if !evidence.valid_at(auction_time::Timestamp::now())
            || evidence.operation_id != operation_id
            || evidence.observed_from < claim.claimed_at
            || evidence.identity.world != claim.world
            || !matches!(
                &evidence.purpose,
                auction_application::settlement::InternalEvidencePurpose::SettlementClaim {
                    obligation_id,
                    claim_token,
                    claim_fencing,
                    state_version
                } if *obligation_id == claim.obligation_id.as_uuid()
                    && *claim_token == claim.claim_token
                    && *claim_fencing == claim.claim_fencing
                    && *state_version == claim.state_version
            )
            || evidence.equipment.as_ref().is_none_or(|equipment| {
                equipment.equipment_id != claim.equipment_id
                    || equipment.external_equipment_id != claim.external_equipment_id
                    || equipment.binding.world != claim.equipment_world
                    || !evidence.identity.same_account(&equipment.binding)
            })
        {
            return Err(FundingError::Unproven);
        }
        crate::lock_mail_runtime_generation(tx, &evidence.identity.runtime)
            .await
            .map_err(evidence_error)?;
    } else if custody.is_some() {
        return Err(FundingError::Invalid);
    }
    crate::database::settlement::funding::lock_funding_worlds(tx).await?;
    let protected =
        crate::database::settlement::funding::funding_requires_authorization(tx).await?;
    locks::before_credentials(tx, claim).await?;
    if let Some((evidence, _)) = custody {
        crate::lock_mail_evidence_credentials(tx, evidence).await.map_err(evidence_error)?;
    }
    locks::after_credentials(tx, claim).await?;
    // 领取之后成交可能已被 Admin 确认撤销：恢复记录与义务已锁定，旧 claim 不能再准备、激活或
    // 满足不再可执行的义务（不变量 2、6）。
    if !super::eligibility::recovery_allows_execution_now(tx, claim.obligation_id.as_uuid())
        .await
        .map_err(sql_error)?
    {
        return Err(FundingError::Conflict);
    }
    if let Some(user_id) = claim.recipient_user_id {
        let current: bool = sqlx::query_scalar(
            r#"SELECT EXISTS(
                   SELECT 1
                   FROM public.users
                   WHERE id=$1
                     AND ($3 OR (username=$2 AND status='active'))
               )"#,
        )
        .bind(user_id)
        .bind(&claim.recipient)
        .bind(claim.state == auction_application::settlement::SettlementFulfillmentState::Enqueued)
        .fetch_one(&mut **tx)
        .await
        .map_err(sql_error)?;
        if !current {
            return Err(FundingError::Conflict);
        }
    }
    Ok(protected)
}
