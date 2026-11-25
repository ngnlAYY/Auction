//! 未付收回的取消写端（`cancel_unpaid`）：未付 COD 退回、无任何到账时封闭原买方交割（7A-2 作单写者）。
//!
//! 保护不变量 1、3、4、6、8：D9 §4–§5.1 的语义——买家从未付款，所以没有卖款、退款与手续费可结，
//! 本操作**零资金分录、零补偿池增量**；唯一的持久效果是追加一行 Sale 封闭事实
//! （`consignment_sale_closures.closure_kind = 'cancelled_unpaid'`，7C-3 定义的只追加表）。
//! 封闭前在 UID 账户锁内复验“确实从未付款”：没有 funded 投影、收款收据与任何本合同的资金分录，付款义务
//! 未满足；买家已确领原件的 Sale 由 000137 的封闭守卫再拒一次。外部结果未知或证据冲突不会走到这里
//! （处置矩阵先留案）。重放同一处置返回“已封闭”，异处置对同一 Sale 的第二次封闭为冲突。

use auction_application::settlement::FundingError;
use sqlx::{Postgres, Row};
use uuid::Uuid;

use super::contract_allocations::sale_facts;
use crate::database::TrackedTransaction as Transaction;
use crate::database::settlement::funding::account::lock_funding_account;
use crate::database::settlement::funding::{lock_funding_worlds, storage};

/// 封闭事实的类型码；与 000137 的 `c7_closure_kind_ck` 一致。
const CANCELLED_UNPAID: &str = "cancelled_unpaid";

/// 一次未付取消：合同与授权它的处置。
#[derive(Debug, Clone, Copy)]
pub(crate) struct CancelUnpaid {
    pub(crate) contract_id: Uuid,
    pub(crate) disposition_id: Uuid,
}

/// 封闭依据：处置 ID 的稳定文本，重放与冲突都以它比较。
fn basis_ref(disposition_id: Uuid) -> String {
    format!("unpaid-recovery:{disposition_id}")
}

/// 在调用方事务内封闭未付 Sale；新写入返回 `true`，同处置重放返回 `false`。
///
/// 调用方负责顶层 Serializable 事务与锁序（本原语先取 funding worlds，再取 UID 账户锁，不提交）。
pub(crate) async fn cancel_unpaid_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    command: &CancelUnpaid,
) -> Result<bool, FundingError> {
    if command.contract_id.is_nil() || command.disposition_id.is_nil() {
        return Err(FundingError::Invalid);
    }
    let sale = sale_facts(tx, command.contract_id).await?;
    lock_funding_worlds(tx).await?;
    lock_funding_account(tx, &sale.funding_uid).await?.ok_or(FundingError::Unproven)?;
    // 只有“关闭原买方交割”的两种处置才授权取消；其余处置（保留、留案、转退款……）不得走这里。
    let authorized: bool = sqlx::query_scalar(
        "SELECT EXISTS (
             SELECT 1 FROM public.consignment_return_dispositions
             WHERE disposition_id = $1 AND contract_id = $2
               AND buyer_right IN ('close_at_limit', 'terminate_unpaid'))",
    )
    .bind(command.disposition_id)
    .bind(command.contract_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(storage)?;
    if !authorized {
        return Err(FundingError::Conflict);
    }
    ensure_never_paid(tx, command.contract_id).await?;
    let inserted = sqlx::query(
        "INSERT INTO public.consignment_sale_closures
            (contract_id, closure_kind, basis_ref, closed_at)
         VALUES ($1, $2, $3, clock_timestamp())
         ON CONFLICT ON CONSTRAINT c7_closure_pk DO NOTHING",
    )
    .bind(command.contract_id)
    .bind(CANCELLED_UNPAID)
    .bind(basis_ref(command.disposition_id))
    .execute(&mut **tx)
    .await
    .map_err(storage)?
    .rows_affected()
        == 1;
    if inserted {
        return Ok(true);
    }
    // 已有封闭事实：只有同一处置的同类封闭是幂等重放，其余（退款封闭、另一处置）都是冲突。
    let existing = sqlx::query(
        "SELECT closure_kind, basis_ref FROM public.consignment_sale_closures
         WHERE contract_id = $1",
    )
    .bind(command.contract_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(storage)?;
    let kind: String = existing.try_get("closure_kind").map_err(storage)?;
    let basis: String = existing.try_get("basis_ref").map_err(storage)?;
    if kind == CANCELLED_UNPAID && basis == basis_ref(command.disposition_id) {
        Ok(false)
    } else {
        Err(FundingError::Conflict)
    }
}

/// “买家从未付款”的资金侧证明：没有 funded 投影、收款收据、本合同的资金分录，付款义务未满足。
async fn ensure_never_paid(
    tx: &mut Transaction<'_, Postgres>,
    contract_id: Uuid,
) -> Result<(), FundingError> {
    let paid: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM public.consignment_funded_sales WHERE contract_id = $1)
             OR EXISTS (SELECT 1 FROM public.consignment_cod_receipts WHERE contract_id = $1)
             OR EXISTS (
                 SELECT 1 FROM public.settlement_funding_entries AS entry
                 JOIN public.settlement_funding_allocations AS allocation
                   ON allocation.id IN (entry.debit_id, entry.credit_id)
                 WHERE allocation.consignment_contract_id = $1)
             OR EXISTS (
                 SELECT 1 FROM public.settlement_obligations AS obligation
                 JOIN public.settlement_obligation_fulfillments AS fulfillment
                   ON fulfillment.obligation_id = obligation.id
                 WHERE obligation.contract_id = $1
                   AND obligation.obligation_kind IN (
                       'winner_equipment_cod', 'winner_payment_carrier_cod')
                   AND fulfillment.execution_state = 'satisfied')",
    )
    .bind(contract_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(storage)?;
    if paid {
        return Err(FundingError::Conflict);
    }
    Ok(())
}
