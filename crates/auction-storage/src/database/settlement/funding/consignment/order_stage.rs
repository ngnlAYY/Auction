//! 资金写口的订单阶段变更：同事务调用统一入口 `record_order_stage_change_in_tx`（事件 + 权利刷新）。
//!
//! 保护不变量 3：订单阶段事件与权利新版本读自同一事务内刚写入的资金事实，与资金事实同提交、同回滚；
//! 调用方只在本次命令确有资金事实变化时调用，重放与无变化命令不再追加事件。

use auction_application::settlement::FundingError;
use sqlx::Postgres;
use uuid::Uuid;

use crate::database::TrackedTransaction as Transaction;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::deposit::primitives::database_now;
use crate::database::consignment::reads::orders::record_order_stage_change_in_tx;

/// 以数据库时钟记录该合同的订单阶段变更；失败时调用方整笔回滚。
pub(super) async fn record_stage_change(
    tx: &mut Transaction<'_, Postgres>,
    contract_id: Uuid,
) -> Result<(), FundingError> {
    record(tx, contract_id).await.map_err(stage_change_error)
}

async fn record(tx: &mut Transaction<'_, Postgres>, contract_id: Uuid) -> Result<(), WriteError> {
    let now = database_now(tx).await?;
    record_order_stage_change_in_tx(tx, contract_id, now).await
}

/// 阶段变更的失败整笔回滚：已确认的 40001 / 40P01 交由序列化重试，其余一律按存储失败关闭。
fn stage_change_error(error: WriteError) -> FundingError {
    if matches!(error, WriteError::RetryConflict) {
        return FundingError::SerializationConflict;
    }
    tracing::warn!(%error, "资金写口的订单阶段变更失败，整笔回滚");
    FundingError::Storage
}
