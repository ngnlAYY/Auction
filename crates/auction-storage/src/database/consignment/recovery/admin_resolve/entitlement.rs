//! `retain_entitlement`：已精确收回的现金退件保留原受益权（D9 §5.1、§6.2）。
//!
//! 保护不变量 1、3、4、6：证据 ID 是该合同的现金退款 / 卖款腿退回处置；资金只由 6B 的
//! `retain_entitlement_in_tx` 改动（原用途 → 同权利 retained，零新增资产，重复调用冲突）。核证据、资金转移、
//! 处置记录、订单阶段事件与回执同事务；资金原语的确定性失败只回滚到保存点再入账拒绝。

use auction_application::consignment::{
    DepositBusinessRejection, OrderResolveReceipt, ResolveOrderCommand,
};
use auction_application::settlement::FundingMutation;
use uuid::Uuid;

use super::ledger::{Flow, OrderLedger, insert_rejection};
use super::{already_resolved, finish, rejection_of, rollback_to_savepoint, savepoint};
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::commands::session::DepositTransaction;
use crate::database::consignment::returns::funding_error;
use crate::database::settlement::funding::consignment::refund_conversion::{
    RetainEntitlement, RetainInTx, retain_entitlement_in_tx,
};

/// 资金命令 ID 的派生域：同一条 Admin 命令永远得到同一个资金命令 ID。
const FUNDING_COMMAND_DOMAIN: &[u8] = b"consignment.resolve.retain.v1";

pub(super) async fn retain_in_tx(
    command: &ResolveOrderCommand,
    ledger: &OrderLedger<'_>,
    tx: &mut DepositTransaction<'_, '_>,
) -> Result<Flow<OrderResolveReceipt>, WriteError> {
    let connection = tx.connection();
    let disposition: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM public.consignment_return_dispositions
                        WHERE disposition_id = $2 AND contract_id = $1
                          AND leg IN ('cash_refund', 'cash_payout'))",
    )
    .bind(command.contract_id)
    .bind(command.evidence_id)
    .fetch_one(&mut *connection)
    .await?;
    if !disposition {
        return insert_rejection(connection, ledger, DepositBusinessRejection::EvidenceUnproven)
            .await;
    }
    if already_resolved(connection, command).await? {
        return insert_rejection(connection, ledger, DepositBusinessRejection::DispositionConflict)
            .await;
    }
    let retain = RetainEntitlement {
        mutation: FundingMutation {
            command_id: Uuid::new_v5(&command.command_id.as_uuid(), FUNDING_COMMAND_DOMAIN),
            request_digest: command.request_hash.as_bytes(),
        },
        contract_id: command.contract_id,
        disposition_id: command.evidence_id,
    };
    savepoint(tx.connection()).await?;
    match retain_entitlement_in_tx(tx.tracked(), &retain).await {
        Ok(RetainInTx::Retained(_)) => finish(tx.connection(), ledger, command).await,
        Ok(RetainInTx::OppositePaymentOpen) => {
            insert_rejection(tx.connection(), ledger, DepositBusinessRejection::OppositePaymentOpen)
                .await
        }
        Err(error) => match rejection_of(&error) {
            Some(rejection) => {
                rollback_to_savepoint(tx.connection()).await?;
                insert_rejection(tx.connection(), ledger, rejection).await
            }
            None => Err(funding_error(error)),
        },
    }
}
