//! 损失原语的 PG 测试窄入口；只在 `test-db` 构建。每个入口在一个顶层事务内调用对应原语。

use auction_application::consignment::{
    ConsignmentError, ListingDelistReason, LossCaseOutcome, LossCaseOutcomeReceipt,
    LossProofReceipt,
};
use auction_domain::CommandId;
use uuid::Uuid;

use super::case::{LossFactRef, OpenedLossCase, open_loss_case};
use super::outcome::{LossOutcomeInput, append_loss_case_outcome};
use super::proof::{LossProofInput, append_loss_proof};
use super::reads::{AdminLossCaseView, SellerLossCaseView, admin_loss_case, seller_loss_case};
use crate::database::DatabaseStore;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::commands::{LockKeys, with_deposit_transaction};
use crate::database::consignment::deposit::primitives::database_now;
use crate::database::pool::TrackedPool;

/// 测试入口的证据输入，字段与存储内部的 `LossProofInput` 一一对应；三个摘要由存储在锁内推导。
pub struct TestLossProof<'a> {
    /// 目标 position。
    pub position_id: Uuid,
    /// 证据来源。
    pub source_kind: &'a str,
    /// 观察身份。
    pub observation_ref: &'a str,
    /// 最后确定位置。
    pub last_known_ref: &'a str,
}

/// 人工结果请求；借给事务闭包，避免闭包捕获调用方的引用。
struct OutcomeRequest<'a> {
    outcome_id: Uuid,
    seller_receipt_id: Uuid,
    case_id: Uuid,
    position_id: Uuid,
    expected_case_version: u64,
    outcome: LossCaseOutcome,
    basis_id: Uuid,
    private_note: &'a ListingDelistReason,
}

fn lock(command_id: Uuid) -> LockKeys<'static> {
    LockKeys { command_id, sellers: &[], equipment: &[] }
}

impl DatabaseStore {
    /// 在一个顶层事务里追加灭失证据。
    pub async fn test_append_loss_proof(
        &self,
        command_id: CommandId,
        proof: &TestLossProof<'_>,
    ) -> Result<LossProofReceipt, ConsignmentError> {
        with_deposit_transaction(self, lock(command_id.as_uuid()), &[], proof, |proof, tx| {
            Box::pin(async move {
                let now = database_now(tx.connection()).await?;
                let input = LossProofInput {
                    position_id: proof.position_id,
                    source_kind: proof.source_kind,
                    observation_ref: proof.observation_ref,
                    last_known_ref: proof.last_known_ref,
                    command_id,
                    now,
                };
                append_loss_proof(tx.connection(), &input).await
            })
        })
        .await
        .map_err(WriteError::into_application)
    }

    /// 在一个顶层事务里为已存在的损失事实开案。
    pub async fn test_open_loss_case(
        &self,
        command_id: CommandId,
        loss_fact_id: Uuid,
        position_id: Uuid,
        seller_user_id: Uuid,
        ids: OpenedLossCase,
    ) -> Result<OpenedLossCase, ConsignmentError> {
        let fact = LossFactRef { loss_fact_id, position_id, seller_user_id };
        with_deposit_transaction(self, lock(command_id.as_uuid()), &[], &fact, |fact, tx| {
            Box::pin(async move {
                let now = database_now(tx.connection()).await?;
                open_loss_case(tx.connection(), fact, command_id, ids, now).await
            })
        })
        .await
        .map_err(WriteError::into_application)
    }

    /// 在一个顶层事务里追加人工结果。
    #[allow(clippy::too_many_arguments, reason = "测试入口逐项对应命令字段，拆分只会增加中间类型")]
    pub async fn test_append_loss_case_outcome(
        &self,
        command_id: CommandId,
        case_id: Uuid,
        position_id: Uuid,
        expected_case_version: u64,
        outcome: LossCaseOutcome,
        basis_id: Uuid,
        private_note: &ListingDelistReason,
        ids: (Uuid, Uuid),
    ) -> Result<LossCaseOutcomeReceipt, ConsignmentError> {
        let (outcome_id, seller_receipt_id) = ids;
        let request = OutcomeRequest {
            outcome_id,
            seller_receipt_id,
            case_id,
            position_id,
            expected_case_version,
            outcome,
            basis_id,
            private_note,
        };
        with_deposit_transaction(self, lock(command_id.as_uuid()), &[], &request, |request, tx| {
            Box::pin(async move {
                let now = database_now(tx.connection()).await?;
                let input = LossOutcomeInput {
                    outcome_id: request.outcome_id,
                    seller_receipt_id: request.seller_receipt_id,
                    case_id: request.case_id,
                    position_id: request.position_id,
                    expected_case_version: request.expected_case_version,
                    outcome: request.outcome,
                    basis_id: request.basis_id,
                    private_note: request.private_note.as_str(),
                    command_id,
                    now,
                };
                append_loss_case_outcome(tx.connection(), &input).await
            })
        })
        .await
        .map_err(WriteError::into_application)
    }

    /// 卖家按 position 读本人的损失回执。
    pub async fn test_seller_loss_case(
        &self,
        seller_user_id: Uuid,
        position_id: Uuid,
    ) -> Result<Option<SellerLossCaseView>, ConsignmentError> {
        let pool: &TrackedPool<sqlx::Postgres> = self.pool();
        seller_loss_case(pool, seller_user_id, position_id)
            .await
            .map_err(WriteError::into_application)
    }

    /// Admin 按 position 读损失案件。
    pub async fn test_admin_loss_case(
        &self,
        position_id: Uuid,
    ) -> Result<Option<AdminLossCaseView>, ConsignmentError> {
        let pool: &TrackedPool<sqlx::Postgres> = self.pool();
        admin_loss_case(pool, position_id).await.map_err(WriteError::into_application)
    }
}
