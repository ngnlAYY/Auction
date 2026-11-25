//! `cancel_unpaid`：买家从未付款的 Sale 由 7A-2 的写端封闭（`consignment_sale_closures(cancelled_unpaid)`）。
//!
//! 保护不变量 1、3、4、6：证据 ID 是该合同“关闭原买方交割”的退回处置（close_at_limit / terminate_unpaid）；
//! 封闭由 `cancel_unpaid_in_tx` 在 UID 账户锁内复验“确实从未付款”后写入，零资金分录；若 7A-2 的
//! `resolve_return` 已按同一处置封闭，则写端重放而不重复。封闭事实、处置记录、订单阶段事件与回执同事务。

use auction_application::consignment::{
    DepositBusinessRejection, OrderResolveReceipt, ResolveOrderCommand,
};

use super::ledger::{Flow, OrderLedger, insert_rejection};
use super::{already_resolved, finish, rejection_of, rollback_to_savepoint, savepoint};
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::commands::session::DepositTransaction;
use crate::database::consignment::returns::funding_error;
use crate::database::settlement::funding::consignment::cancel_unpaid::{
    CancelUnpaid, cancel_unpaid_in_tx,
};

pub(super) async fn cancel_in_tx(
    command: &ResolveOrderCommand,
    ledger: &OrderLedger<'_>,
    tx: &mut DepositTransaction<'_, '_>,
) -> Result<Flow<OrderResolveReceipt>, WriteError> {
    let connection = tx.connection();
    let closing: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM public.consignment_return_dispositions
                        WHERE disposition_id = $2 AND contract_id = $1
                          AND buyer_right IN ('close_at_limit', 'terminate_unpaid'))",
    )
    .bind(command.contract_id)
    .bind(command.evidence_id)
    .fetch_one(&mut *connection)
    .await?;
    if !closing {
        return insert_rejection(connection, ledger, DepositBusinessRejection::EvidenceUnproven)
            .await;
    }
    if already_resolved(connection, command).await? {
        return insert_rejection(connection, ledger, DepositBusinessRejection::DispositionConflict)
            .await;
    }
    savepoint(tx.connection()).await?;
    let cancel =
        CancelUnpaid { contract_id: command.contract_id, disposition_id: command.evidence_id };
    match cancel_unpaid_in_tx(tx.tracked(), &cancel).await {
        Ok(_) => finish(tx.connection(), ledger, command).await,
        Err(error) => match rejection_of(&error) {
            Some(rejection) => {
                rollback_to_savepoint(tx.connection()).await?;
                insert_rejection(tx.connection(), ledger, rejection).await
            }
            None => Err(funding_error(error)),
        },
    }
}
