//! 收款分账：合同键下的分配、过渡用途的借出 / 发行 / 转出，以及手续费、本金与 funded 事实。
//!
//! 保护不变量 3、6：已含 A 从原 opening allocation 借出、未含 U 由本笔付款 source 发行，二者等额贷入
//! 同合同专用的不可派发过渡用途；随后只转出 S / F / 本金，过渡用途余额回零。三条合计等式另有数据库
//! 延迟约束；寄售对补偿池的余额与账本增量恒为零（决定 19）。

use auction_application::settlement::{CertifyCod, CodLeg, FundingError, FundingMutation};
use auction_domain::consignment::funding::{TransitOutput, TransitPlan};
use sqlx::{Postgres, Row};
use uuid::Uuid;

use super::receipt::{funding_sql_error, mode_code};
use super::record_only::principal_destination;
use crate::database::TrackedTransaction as Transaction;
use crate::database::settlement::funding::ledger::{issue, transfer};
use crate::database::settlement::funding::storage;

/// 取得（必要时创建）合同键下的分配并锁行；插入、冲突与锁读使用同一四列键，
/// 两个可空 FK 均 `IS NOT DISTINCT FROM`（D9 §4.3）。拍卖的 `ledger::allocation` 只认合同为空的行，不能复用。
pub(super) async fn contract_allocation(
    tx: &mut Transaction<'_, Postgres>,
    uid: &str,
    purpose: &str,
    contract_id: Uuid,
    obligation: Option<Uuid>,
) -> Result<Uuid, FundingError> {
    sqlx::query(
        r#"INSERT INTO public.settlement_funding_allocations
               (id, uid, purpose, lot_id, consignment_contract_id, obligation_id, amount)
           VALUES ($1, $2, $3, NULL, $4, $5, 0)
           ON CONFLICT (uid, purpose, lot_id, consignment_contract_id) DO NOTHING"#,
    )
    .bind(Uuid::new_v4())
    .bind(uid)
    .bind(purpose)
    .bind(contract_id)
    .bind(obligation)
    .execute(&mut **tx)
    .await
    .map_err(funding_sql_error)?;
    let row = sqlx::query(
        r#"SELECT id, obligation_id
           FROM public.settlement_funding_allocations
           WHERE uid = $1 AND purpose = $2 AND lot_id IS NULL
             AND consignment_contract_id IS NOT DISTINCT FROM $3
           FOR UPDATE"#,
    )
    .bind(uid)
    .bind(purpose)
    .bind(contract_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(storage)?;
    if row.try_get::<Option<Uuid>, _>("obligation_id").map_err(storage)? != obligation {
        return Err(FundingError::Conflict);
    }
    row.try_get("id").map_err(storage)
}

/// 过渡用途与三段转出的目标分配（已创建并锁行）。
pub(super) struct DistributionTargets {
    transit: Uuid,
    /// 转出目标：（分配、金额、是否 fee）。
    outputs: Vec<(Uuid, i64, bool)>,
}

/// 创建并锁定本笔分账涉及的全部合同分配（锁序：obligations 之后、deliveries 之前的 allocations 段）。
pub(super) async fn resolve_targets(
    tx: &mut Transaction<'_, Postgres>,
    command: &CertifyCod,
    plan: &TransitPlan,
    payout_obligation_id: Uuid,
) -> Result<DistributionTargets, FundingError> {
    let uid = command.receipt.uid.as_str();
    let transit =
        contract_allocation(tx, uid, "consignment_transit", command.contract_id, None).await?;
    let mut outputs = Vec::with_capacity(plan.outputs.len());
    for output in &plan.outputs {
        let target = match output {
            TransitOutput::Seller(amount) => (
                contract_allocation(
                    tx,
                    uid,
                    "seller",
                    command.contract_id,
                    Some(payout_obligation_id),
                )
                .await?,
                *amount,
                false,
            ),
            TransitOutput::Fee(amount) => (
                contract_allocation(tx, uid, "fee", command.contract_id, None).await?,
                *amount,
                true,
            ),
            TransitOutput::Principal { amount, mode } => {
                (principal_destination(tx, uid, *mode, command.contract_id).await?, *amount, false)
            }
        };
        outputs.push(target);
    }
    Ok(DistributionTargets { transit, outputs })
}

/// 执行过渡用途的三段计划；返回实际贷入 fee 的那笔转移分录 ID（F=0 为 `None`）。
///
/// `borrows` 是按原 allocation 汇总的借出额（合计 = A）；`source_id` 是本笔付款 source（额度 = U）。
pub(super) async fn distribute(
    tx: &mut Transaction<'_, Postgres>,
    command: &CertifyCod,
    plan: &TransitPlan,
    source_id: Uuid,
    borrows: &[(Uuid, i64)],
    targets: &DistributionTargets,
) -> Result<Option<Uuid>, FundingError> {
    let uid = command.receipt.uid.as_str();
    let mutation = &command.mutation;
    let transit = targets.transit;
    // 未含部分由本笔付款 source 发行；已含部分从原 opening 借出，二者等额贷入过渡用途。
    issue(tx, uid, mutation, source_id, transit, plan.new_issue).await?;
    for (allocation_id, amount) in borrows {
        transfer(tx, uid, mutation, *allocation_id, transit, *amount).await?;
    }
    let mut fee_entry = None;
    for (target, amount, is_fee) in &targets.outputs {
        transfer(tx, uid, mutation, transit, *target, *amount).await?;
        if *is_fee {
            fee_entry = Some(fee_entry_id(tx, mutation, transit, *target).await?);
        }
    }
    Ok(fee_entry)
}

/// 找到本命令里 过渡用途 → fee 的那一笔转移分录。
async fn fee_entry_id(
    tx: &mut Transaction<'_, Postgres>,
    mutation: &FundingMutation,
    transit: Uuid,
    fee: Uuid,
) -> Result<Uuid, FundingError> {
    sqlx::query_scalar(
        r#"SELECT id FROM public.settlement_funding_entries
           WHERE command_id = $1 AND debit_id = $2 AND credit_id = $3 AND kind = 'transfer'"#,
    )
    .bind(mutation.command_id)
    .bind(transit)
    .bind(fee)
    .fetch_one(&mut **tx)
    .await
    .map_err(storage)
}

/// F>0 时恰一条 earned，引用本笔收据与实际贷入 fee 的分录；F=0 不落任何行（不占位）。
pub(super) async fn write_fee_earned(
    tx: &mut Transaction<'_, Postgres>,
    command: &CertifyCod,
    receipt_source_id: Uuid,
    fee_entry_id: Option<Uuid>,
) -> Result<(), FundingError> {
    let Some(fee_entry_id) = fee_entry_id else {
        return Ok(());
    };
    sqlx::query(
        r#"INSERT INTO public.consignment_fee_facts
               (id, contract_id, kind, amount, receipt_source_id, fee_entry_id)
           VALUES ($1, $2, 'earned', $3, $4, $5)"#,
    )
    .bind(Uuid::new_v4())
    .bind(command.contract_id)
    .bind(command.quote.fee)
    .bind(receipt_source_id)
    .bind(fee_entry_id)
    .execute(&mut **tx)
    .await
    .map_err(funding_sql_error)?;
    Ok(())
}

/// isekai 载体腿付款时的本金结论 `payment_principal`；与 `rts_principal` 互斥，由唯一键保证。
pub(super) async fn write_principal_fact(
    tx: &mut Transaction<'_, Postgres>,
    command: &CertifyCod,
    round: i32,
) -> Result<(), FundingError> {
    if command.receipt.leg != CodLeg::Carrier {
        return Ok(());
    }
    sqlx::query(
        r#"INSERT INTO public.consignment_principal_facts
               (id, contract_id, carrier_obligation_id, original_delivery_id, round, conclusion,
                mode, amount)
           VALUES ($1, $2, $3, $4, $5, 'payment_principal', $6, 1)"#,
    )
    .bind(Uuid::new_v4())
    .bind(command.contract_id)
    .bind(command.payment_obligation_id)
    .bind(command.original_delivery_id)
    .bind(round)
    .bind(mode_code(command.mode))
    .execute(&mut **tx)
    .await
    .map_err(funding_sql_error)?;
    Ok(())
}

/// funded 投影：一份实收一个合同，复合绑定原收据。
pub(super) async fn write_funded_sale(
    tx: &mut Transaction<'_, Postgres>,
    command: &CertifyCod,
    receipt_source_id: Uuid,
) -> Result<(), FundingError> {
    sqlx::query(
        r#"INSERT INTO public.consignment_funded_sales
               (contract_id, uid, payment_obligation_id, receipt_source_id)
           VALUES ($1, $2, $3, $4)"#,
    )
    .bind(command.contract_id)
    .bind(&command.receipt.uid)
    .bind(command.payment_obligation_id)
    .bind(receipt_source_id)
    .execute(&mut **tx)
    .await
    .map_err(funding_sql_error)?;
    Ok(())
}
