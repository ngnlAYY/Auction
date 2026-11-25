//! Admin 损失案件端口的存储实现：读案件与追加人工结果。
//!
//! 保护不变量 1、2、8：人工结果经命令账本仲裁（见 `commands::record_outcome`），在一个顶层事务内
//! 与结果行、卖家回执同提；并发由 Serializable 与 (case_id, case_version) 唯一约束仲裁，落败方重试后读到
//! 新版本判 `stale_version`。同命令同请求返回首次回执，同命令异请求为 `command_id_conflict`。

use auction_application::consignment::{
    AdminLossCaseRecord, AdminLossOutcomeRecord, ConsignmentError, ConsignmentFuture,
    LossCaseAdmin, LossCaseOutcomeReceipt, LossCaseState, LossReconcileRecord,
    RecordLossCaseOutcomeCommand,
};
use sqlx::Postgres;
use uuid::Uuid;

use super::commands::record_outcome;
use super::reads::{AdminLossCaseView, admin_loss_case};
use super::reconcile::open_reconciles;
use crate::database::DatabaseStore;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::pool::TrackedPool;

impl LossCaseAdmin for DatabaseStore {
    fn admin_loss_case(
        &self,
        position_id: Uuid,
    ) -> ConsignmentFuture<'_, Option<AdminLossCaseRecord>> {
        Box::pin(async move {
            let pool: &TrackedPool<Postgres> = self.pool();
            let view =
                admin_loss_case(pool, position_id).await.map_err(WriteError::into_application)?;
            view.map(record).transpose()
        })
    }

    fn open_loss_reconciles(
        &self,
        position_id: Uuid,
    ) -> ConsignmentFuture<'_, Vec<LossReconcileRecord>> {
        Box::pin(async move {
            open_reconciles(self, position_id).await.map_err(WriteError::into_application)
        })
    }

    fn record_loss_case_outcome(
        &self,
        command: RecordLossCaseOutcomeCommand,
    ) -> ConsignmentFuture<'_, LossCaseOutcomeReceipt> {
        Box::pin(record_outcome(self, command))
    }
}

/// 把存储视图转成应用记录；未知状态说明库内数据损坏，失败关闭。
fn record(view: AdminLossCaseView) -> Result<AdminLossCaseRecord, ConsignmentError> {
    let state = |text: &str| LossCaseState::parse(text).ok_or(ConsignmentError::Unavailable);
    let outcomes = view
        .outcomes
        .into_iter()
        .map(|outcome| {
            Ok(AdminLossOutcomeRecord {
                outcome_id: outcome.outcome_id,
                case_version: outcome.case_version,
                state: state(&outcome.outcome)?,
                basis_id: outcome.basis_id,
                private_note: outcome.private_note,
                occurred_at: outcome.occurred_at,
                seller_receipt_id: outcome.seller_receipt_id,
            })
        })
        .collect::<Result<Vec<_>, ConsignmentError>>()?;
    Ok(AdminLossCaseRecord {
        position_id: view.position_id,
        case_id: view.case_id,
        loss_fact_id: view.loss_fact_id,
        proof_id: view.proof_id,
        proof_source_kind: view.proof_source_kind,
        proof_digest: view.proof_digest,
        listing_id: view.listing_id,
        seller_user_id: view.seller_user_id,
        generation: view.generation,
        case_version: view.case_version,
        state: state(&view.state)?,
        outcomes,
    })
}
