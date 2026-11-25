//! 退款类处置（refund_returned / refund_lost / refund_season）：三个动作共用同一个 6B 写端。
//!
//! 保护不变量 1、3、4、6、8：资金只由 `convert_refund` 这一个写者改动（同命令 ID 同请求返回原结果）；
//! 核证据、锁内复验相反付款、资金转换、`refund_closed` 封闭事实、处置记录、订单阶段事件与账本回执在同一个
//! Serializable 事务里提交，所以不存在“资金已转换而阶段事件缺失”的崩溃窗口。卖款可能已发或已预留时写者在
//! UID 账户锁内返回 `OppositePaymentOpen`：不留任何资金痕迹，确定拒绝入账。

use auction_application::consignment::{
    DepositBusinessRejection, OrderResolveAction, OrderResolveReceipt, ResolveOrderCommand,
};
use auction_application::settlement::FundingMutation;
use uuid::Uuid;

use super::ledger::{Flow, OrderLedger, insert_rejection};
use super::{already_resolved, finish, rejection_of, rollback_to_savepoint, savepoint};
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::commands::session::DepositTransaction;
use crate::database::consignment::recovery::refund_closure::{
    record_refund_closure, refund_command,
};
use crate::database::consignment::returns::funding_error;
use crate::database::settlement::funding::consignment::refund_conversion::{
    RefundInTx, convert_refund_in_tx,
};

/// 6B 写端命令 ID 的派生域：同一条 Admin 命令永远得到同一个资金命令 ID，重试不会产生第二笔转换。
const FUNDING_COMMAND_DOMAIN: &[u8] = b"consignment.resolve.refund.v1";

/// 各退款动作的证据判据（`$1` 合同、`$2` 证据 ID）；证据必须属于该合同，否则别的合同的真实证据
/// 就能给本合同退款（不变量 4、6）。
fn evidence_sql(action: OrderResolveAction) -> Option<&'static str> {
    Some(match action {
        // 证据 ID 是退件消费：原件已被精确收回并消费。
        OrderResolveAction::RefundReturned => {
            "SELECT EXISTS (SELECT 1 FROM public.consignment_return_consumptions
                            WHERE id = $2 AND contract_id = $1 AND leg = 'original')"
        }
        // 证据 ID 是灭失证据：独立的灭失证据，且属于该合同的 position。
        OrderResolveAction::RefundLost => {
            "SELECT EXISTS (SELECT 1 FROM public.consignment_loss_proofs AS proof
                            JOIN public.consignment_contracts AS contract
                              ON contract.position_id = proof.position_id
                            WHERE proof.proof_id = $2 AND contract.contract_id = $1)"
        }
        // 证据 ID 是该合同的 position：它已有 season_ended 代管事件（季末事件不带邮件证明，
        // 见 c7_custody_event_season_ended_ck，所以不能用证明 ID 指认）。
        OrderResolveAction::RefundSeason => {
            "SELECT EXISTS (SELECT 1 FROM public.consignment_custody_events AS event
                            JOIN public.consignment_contracts AS contract
                              ON contract.position_id = event.position_id
                            WHERE contract.contract_id = $1 AND event.position_id = $2
                              AND event.event_kind = 'season_ended')"
        }
        _ => return None,
    })
}

fn funding_mutation(command: &ResolveOrderCommand) -> FundingMutation {
    FundingMutation {
        command_id: Uuid::new_v5(&command.command_id.as_uuid(), FUNDING_COMMAND_DOMAIN),
        request_digest: command.request_hash.as_bytes(),
    }
}

/// 退款类处置的事务体（调用方已完成账本准入并持有 funding worlds 锁）。
pub(super) async fn refund_in_tx(
    command: &ResolveOrderCommand,
    ledger: &OrderLedger<'_>,
    tx: &mut DepositTransaction<'_, '_>,
) -> Result<Flow<OrderResolveReceipt>, WriteError> {
    let Some(sql) = evidence_sql(command.action) else {
        return insert_rejection(
            tx.connection(),
            ledger,
            DepositBusinessRejection::ActionNotAllowed,
        )
        .await;
    };
    let connection = tx.connection();
    let convert =
        refund_command(connection, command.contract_id, funding_mutation(command)).await?;
    // 只有 Sale 有买家退款义务；Withdrawal 或缺义务的合同不能退款。
    let Some(convert) = convert else {
        return insert_rejection(connection, ledger, DepositBusinessRejection::ActionNotAllowed)
            .await;
    };
    // 买家已确领原件：再退款就是双付，证据互相矛盾，保留给人工核对（不变量 4）。
    let claimed: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM public.consignment_handoff_proofs AS handoff
                        JOIN public.settlement_obligations AS obligation
                          ON obligation.id = handoff.obligation_id
                        WHERE obligation.contract_id = $1)",
    )
    .bind(command.contract_id)
    .fetch_one(&mut *connection)
    .await?;
    if claimed {
        return insert_rejection(connection, ledger, DepositBusinessRejection::EvidenceConflict)
            .await;
    }
    let proven: bool = sqlx::query_scalar(sql)
        .bind(command.contract_id)
        .bind(command.evidence_id)
        .fetch_one(&mut *connection)
        .await?;
    if !proven {
        return insert_rejection(connection, ledger, DepositBusinessRejection::EvidenceUnproven)
            .await;
    }
    if already_resolved(connection, command).await? {
        return insert_rejection(connection, ledger, DepositBusinessRejection::DispositionConflict)
            .await;
    }
    let basis = format!("refund-conversion:{}", convert.mutation.command_id);
    savepoint(tx.connection()).await?;
    match convert_refund_in_tx(tx.tracked(), &convert).await {
        Ok(RefundInTx::Converted(_)) => {
            record_refund_closure(tx.tracked(), command.contract_id, &basis)
                .await
                .map_err(funding_error)?;
            finish(tx.connection(), ledger, command).await
        }
        Ok(RefundInTx::OppositePaymentOpen) => {
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
