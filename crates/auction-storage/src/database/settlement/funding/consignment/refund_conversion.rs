//! 已证退款原因下，把本合同的卖款 S 与手续费 F 转为买家退款 P，并冲回本笔手续费。
//!
//! 保护不变量 1、3、4、6、8：S + F = P 全额转换且至多一次（funded 投影只前进一步、reversed 唯一）；
//! 卖款可能已发（非 available、已派发或在途）时不转换，留应退债权与相反支付冲突，禁止双派；
//! F=0 时没有 fee 分录、earned 与 reversed，只有 seller→refund 一笔；寄售对补偿池零增量（决定 19）。
//!
//! 同一写端另有两个入口服务 Admin 的 resolve（D9 §6.2 “convert_refund/retain_entitlement”）：
//! `convert_refund_in_tx` 在调用方事务内转换，并在 UID 账户锁内复验“相反付款未了结”，使退款与订单阶段事件
//! 同事务（不变量 3、4）；`retain_entitlement_in_tx` 把已精确收回的现金退件恢复到原受益用途后的金额转入
//! 同权利的 retained 用途（`settle_cash` 的 retained 迁移同一条白名单边），零新增资产。

use auction_application::settlement::{
    ConvertRefund, FundingError, FundingMutation, RefundReceipt,
};
use auction_domain::consignment::funding::{FeeReversal, RefundConversion, refund_conversion};
use sqlx::{Postgres, Row};
use uuid::Uuid;

use super::commands::replay_command;
use super::contract_allocations::{contract_allocation, locked_allocation, sale_facts};
use super::fee::write_fee_reversed_in_tx;
use super::order_stage::record_stage_change;
use super::transaction::run_serializable;
use crate::DatabaseStore;
use crate::database::TrackedTransaction as Transaction;
use crate::database::settlement::funding::account::lock_funding_account;
use crate::database::settlement::funding::ledger::{complete, transfer};
use crate::database::settlement::funding::{invalid, lock_funding_worlds, storage};

const ACTION: &str = "consignment_refund_convert";
const RETAIN_ACTION: &str = "consignment_entitlement_retain";

/// 事务内转换的结果。
#[derive(Debug)]
pub(crate) enum RefundInTx {
    /// 已转换（或同命令原样重放）。
    Converted(RefundReceipt),
    /// 锁内复验发现卖款可能已发或已预留：没有任何资金写入，调用方确定拒绝。
    OppositePaymentOpen,
}

/// 转换的事务入口：同命令 ID 同请求返回原结果；同合同再次转换为冲突（首次退款恰一笔 reversed）。
///
/// 新转换在同事务追加订单阶段事件并刷新权利（不变量 3）；重放不再追加。`convert_refund_in_tx` 不追加，
/// 由它的调用方在自己的事务里统一调用阶段变更入口。
pub(crate) async fn convert_refund(
    store: &DatabaseStore,
    command: ConvertRefund,
) -> Result<RefundReceipt, FundingError> {
    if command.contract_id.is_nil() || command.refund_obligation_id.is_nil() {
        return Err(FundingError::Invalid);
    }
    let conversion = refund_conversion(command.price, command.quote).map_err(invalid)?;
    run_serializable(store, &command, move |tx, command| {
        Box::pin(async move {
            match convert_in_tx(tx, command, conversion, false).await? {
                RefundInTx::Converted(receipt) => {
                    if !receipt.replayed {
                        record_stage_change(tx, command.contract_id).await?;
                    }
                    Ok(receipt)
                }
                // 本入口不复验相反付款，不会得到该结果；失败关闭为冲突。
                RefundInTx::OppositePaymentOpen => Err(FundingError::Conflict),
            }
        })
    })
    .await
}

/// 在调用方的顶层 Serializable 事务内转换；调用方负责提交、回滚与锁序（本原语自取 funding worlds 与 UID 锁）。
///
/// 与 `convert_refund` 的差别只有一处：UID 账户锁之后、读取投影之前，复验卖款腿没有 Send 尝试且卖款分配仍
/// 原样可用（不变量 4）。预读不能代替这一步：预读与锁之间卖款可能被派发或预留，竞争由 Serializable 的
/// 读写依赖与该复验共同关闭。
pub(crate) async fn convert_refund_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    command: &ConvertRefund,
) -> Result<RefundInTx, FundingError> {
    if command.contract_id.is_nil() || command.refund_obligation_id.is_nil() {
        return Err(FundingError::Invalid);
    }
    let conversion = refund_conversion(command.price, command.quote).map_err(invalid)?;
    convert_in_tx(tx, command, conversion, true).await
}

/// 卖款可能已发：卖款腿已有 Send 尝试，或卖款分配已不在 `available`（已预留、在途、未知或已支出）。
async fn opposite_payment_open_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    contract_id: Uuid,
) -> Result<bool, FundingError> {
    sqlx::query_scalar(
        "SELECT EXISTS (
             SELECT 1 FROM public.consignment_dispatch_attempts
             WHERE contract_id = $1 AND leg = 'cash_payout')
         OR EXISTS (
             SELECT 1 FROM public.settlement_funding_allocations
             WHERE consignment_contract_id = $1 AND purpose = 'seller'
               AND (state <> 'available' OR delivery_id IS NOT NULL))",
    )
    .bind(contract_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(storage)
}

async fn convert_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    command: &ConvertRefund,
    conversion: RefundConversion,
    guard_opposite: bool,
) -> Result<RefundInTx, FundingError> {
    // 转换金额必须就是成交时冻结的那一份，不是调用方自带的另一份报价。
    let sale = sale_facts(tx, command.contract_id).await?;
    if sale.price != command.price.value()
        || sale.fee != command.quote.fee
        || sale.seller_amount != command.quote.seller_amount
    {
        return Err(FundingError::Conflict);
    }
    lock_funding_worlds(tx).await?;
    let account =
        lock_funding_account(tx, &sale.funding_uid).await?.ok_or(FundingError::Unproven)?;
    let uid = account.uid().to_owned();
    if replay_command(tx, &uid, &command.mutation, ACTION, command.refund_obligation_id).await? {
        let fee_reversal = replayed_reversal(tx, command).await?;
        return Ok(RefundInTx::Converted(RefundReceipt {
            conversion,
            fee_reversal,
            replayed: true,
        }));
    }
    if guard_opposite && opposite_payment_open_in_tx(tx, command.contract_id).await? {
        return Ok(RefundInTx::OppositePaymentOpen);
    }
    let disposition: String = sqlx::query_scalar(
        "SELECT disposition FROM public.consignment_funded_sales WHERE contract_id=$1 FOR UPDATE",
    )
    .bind(command.contract_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage)?
    .ok_or(FundingError::Unproven)?;
    // 只有 funded 能转换：已转换过或原件已确领都不再转换。
    if disposition != "funded" {
        return Err(FundingError::Conflict);
    }
    // 锁序 obligations → allocations（D9 §1.3）：退款义务行在用途分配之前锁住，转换后同事务激活。
    let refund_state: Option<String> = sqlx::query_scalar(
        r#"SELECT business_state FROM public.settlement_obligations
           WHERE id=$1 AND contract_id=$2 AND obligation_kind='buyer_refund'
           FOR UPDATE"#,
    )
    .bind(command.refund_obligation_id)
    .bind(command.contract_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage)?;
    // 只有尚未激活的或有退款能被转换；已激活说明另有退款事实，失败关闭（不变量 6）。
    if refund_state.as_deref() != Some("contingent") {
        return Err(FundingError::Conflict);
    }
    let seller = locked_allocation(tx, &uid, "seller", command.contract_id)
        .await?
        .ok_or(FundingError::Unproven)?;
    // 卖款必须原样在账：已预留、在途、未知或已支出都可能已发，转换会形成双派。
    if seller.state != "available"
        || seller.delivery_id.is_some()
        || seller.amount != conversion.from_seller
    {
        return Err(FundingError::Conflict);
    }
    let fee = if conversion.from_fee > 0 {
        let row = locked_allocation(tx, &uid, "fee", command.contract_id)
            .await?
            .ok_or(FundingError::Unproven)?;
        if row.state != "available" || row.amount != conversion.from_fee {
            return Err(FundingError::Conflict);
        }
        Some(row)
    } else {
        None
    };
    let refund = contract_allocation(
        tx,
        &uid,
        "refund",
        command.contract_id,
        Some(command.refund_obligation_id),
    )
    .await?;
    let refund_row = locked_allocation(tx, &uid, "refund", command.contract_id)
        .await?
        .ok_or(FundingError::Storage)?;
    if refund_row.amount != 0 || refund_row.state != "available" {
        return Err(FundingError::Conflict);
    }
    transfer(tx, &uid, &command.mutation, seller.id, refund, conversion.from_seller).await?;
    let fee_entry = match &fee {
        Some(fee) => {
            transfer(tx, &uid, &command.mutation, fee.id, refund, conversion.from_fee).await?;
            Some(fee_to_refund_entry(tx, &uid, command.mutation.command_id, fee.id, refund).await?)
        }
        None => None,
    };
    let fee_reversal = write_fee_reversed_in_tx(
        tx,
        command.contract_id,
        command.quote,
        command.refund_obligation_id,
        fee_entry,
    )
    .await?;
    let advanced = sqlx::query(
        r#"UPDATE public.consignment_funded_sales
           SET disposition='refund_converted', version=version+1
           WHERE contract_id=$1 AND disposition='funded'"#,
    )
    .bind(command.contract_id)
    .execute(&mut **tx)
    .await
    .map_err(storage)?;
    if advanced.rows_affected() != 1 {
        return Err(FundingError::Conflict);
    }
    activate_refund(tx, command).await?;
    complete(tx, &account, &command.mutation, ACTION, command.refund_obligation_id).await?;
    Ok(RefundInTx::Converted(RefundReceipt { conversion, fee_reversal, replayed: false }))
}

/// 退款义务 `contingent → ready` 并清除履约行的未激活标记（PIWP；与拍卖恢复激活退款同一写法）：P 已转入退款用途，
/// 退款首轮由下游派发轮生产者建立，派发 executor 只领 `ready` 义务。
///
/// 保护不变量 3、6：激活与资金转换同事务，转换回滚则义务仍为或有；迁移 000110 的延迟约束在提交时拒绝漏清标记
/// 的激活。行已在本事务锁住（上方的 `FOR UPDATE`），未命中即状态被并发改动，失败关闭。
async fn activate_refund(
    tx: &mut Transaction<'_, Postgres>,
    command: &ConvertRefund,
) -> Result<(), FundingError> {
    let activated = sqlx::query(
        r#"UPDATE public.settlement_obligations
           SET business_state='ready', state_version=state_version+1, updated_at=clock_timestamp()
           WHERE id=$1 AND contract_id=$2 AND obligation_kind='buyer_refund'
             AND business_state='contingent'"#,
    )
    .bind(command.refund_obligation_id)
    .bind(command.contract_id)
    .execute(&mut **tx)
    .await
    .map_err(storage)?;
    if activated.rows_affected() != 1 {
        return Err(FundingError::Conflict);
    }
    sqlx::query(
        r#"UPDATE public.settlement_obligation_fulfillments SET dormant=false
           WHERE obligation_id=$1 AND dormant"#,
    )
    .bind(command.refund_obligation_id)
    .execute(&mut **tx)
    .await
    .map_err(storage)?;
    Ok(())
}

/// 一次权利保留：合同与授权它的已证退回处置（`consignment_return_dispositions`）。
#[derive(Debug, Clone)]
pub(crate) struct RetainEntitlement {
    pub(crate) mutation: FundingMutation,
    pub(crate) contract_id: Uuid,
    pub(crate) disposition_id: Uuid,
}

/// 事务内保留的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RetainInTx {
    /// 已转入 retained（或同命令原样重放）。
    Retained(RetainReceipt),
    /// 锁内复验发现卖款可能已发或已预留：没有任何资金写入，调用方确定拒绝。
    OppositePaymentOpen,
}

/// 保留结果：转入 retained 的金额与是否重放。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RetainReceipt {
    pub(crate) amount: i64,
    pub(crate) replayed: bool,
}

/// 在调用方的顶层 Serializable 事务内保留原受益权（D9 §5.1 “退款、卖款或撤回件被退：原受益权继续”）。
///
/// 保护不变量 1、4、6：授权来自该合同的现金退款 / 卖款腿退回处置，且该腿的精确 Take 已把原支出一次全额
/// 回补（`consignment_funding_returns`）；只转移仍 `available` 且未绑定派发的原用途金额，已预留、在途或
/// 已支出的钱不能留存；转入 retained 后卖款 / 退款腿不再有可重复支出的余额（`convert_refund` 与再派发都会
/// 因余额不足而冲突），所以同一笔钱不会既留存又支出。同命令 ID 同请求重放原结果，同 ID 异请求冲突。
pub(crate) async fn retain_entitlement_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    command: &RetainEntitlement,
) -> Result<RetainInTx, FundingError> {
    if command.contract_id.is_nil() || command.disposition_id.is_nil() {
        return Err(FundingError::Invalid);
    }
    let sale = sale_facts(tx, command.contract_id).await?;
    lock_funding_worlds(tx).await?;
    let account =
        lock_funding_account(tx, &sale.funding_uid).await?.ok_or(FundingError::Unproven)?;
    let uid = account.uid().to_owned();
    // 回补金额以资金回补事实为准；放在重放之前读取，使重放也能给出原金额。
    let returned = returned_cash(tx, command).await?;
    if replay_command(tx, &uid, &command.mutation, RETAIN_ACTION, command.disposition_id).await? {
        return Ok(RetainInTx::Retained(RetainReceipt { amount: returned.amount, replayed: true }));
    }
    // 退款腿被退时，相反方向是卖款：与 `convert_refund_in_tx` 同一位置（UID 锁与重放之后、写入之前）复验，
    // 卖款可能已发或已预留则不留存（D9 §6.2 拒 opposite_payment_open）。卖款腿自己的退件不适用：该腿本来就
    // 有 cash_payout 派发尝试，退回的正是它。`funding_boundary` 在 convert 路径没有对应检查（opening 边界由
    // 付款认证写端把关），本入口同样没有。
    if returned.purpose == "refund" && opposite_payment_open_in_tx(tx, command.contract_id).await? {
        return Ok(RetainInTx::OppositePaymentOpen);
    }
    let original = locked_allocation(tx, &uid, returned.purpose, command.contract_id)
        .await?
        .ok_or(FundingError::Unproven)?;
    if original.state != "available"
        || original.delivery_id.is_some()
        || original.amount < returned.amount
    {
        return Err(FundingError::Conflict);
    }
    let retained = contract_allocation(tx, &uid, "retained", command.contract_id, None).await?;
    transfer(tx, &uid, &command.mutation, original.id, retained, returned.amount).await?;
    complete(tx, &account, &command.mutation, RETAIN_ACTION, command.disposition_id).await?;
    Ok(RetainInTx::Retained(RetainReceipt { amount: returned.amount, replayed: false }))
}

/// 处置对应的现金腿回补事实：原用途与已回补金额。
struct ReturnedCash {
    purpose: &'static str,
    amount: i64,
}

async fn returned_cash(
    tx: &mut Transaction<'_, Postgres>,
    command: &RetainEntitlement,
) -> Result<ReturnedCash, FundingError> {
    let row = sqlx::query(
        r#"SELECT disposition.leg, funding_return.amount
           FROM public.consignment_return_dispositions AS disposition
           JOIN public.consignment_funding_returns AS funding_return
             ON funding_return.consumption_id = disposition.consumption_id
            AND funding_return.contract_id = disposition.contract_id
            AND funding_return.leg = disposition.leg
           WHERE disposition.disposition_id=$1 AND disposition.contract_id=$2
             AND disposition.leg IN ('cash_refund', 'cash_payout')"#,
    )
    .bind(command.disposition_id)
    .bind(command.contract_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage)?
    .ok_or(FundingError::Unproven)?;
    let leg: String = row.try_get("leg").map_err(storage)?;
    Ok(ReturnedCash {
        purpose: if leg == "cash_refund" { "refund" } else { "seller" },
        amount: row.try_get("amount").map_err(storage)?,
    })
}

/// 刚写入的 fee→refund 转移分录 ID：以本命令 ID、两端分配与种类唯一定位。
async fn fee_to_refund_entry(
    tx: &mut Transaction<'_, Postgres>,
    uid: &str,
    command_id: Uuid,
    fee_allocation: Uuid,
    refund_allocation: Uuid,
) -> Result<Uuid, FundingError> {
    sqlx::query_scalar(
        r#"SELECT id FROM public.settlement_funding_entries
           WHERE uid=$1 AND command_id=$2 AND debit_id=$3 AND credit_id=$4 AND kind='transfer'"#,
    )
    .bind(uid)
    .bind(command_id)
    .bind(fee_allocation)
    .bind(refund_allocation)
    .fetch_one(&mut **tx)
    .await
    .map_err(storage)
}

/// 重放：F>0 时读回原 reversed 事实，F=0 没有手续费冲回。
async fn replayed_reversal(
    tx: &mut Transaction<'_, Postgres>,
    command: &ConvertRefund,
) -> Result<Option<FeeReversal>, FundingError> {
    if command.quote.fee == 0 {
        return Ok(None);
    }
    let row = sqlx::query(
        r#"SELECT amount, receipt_source_id
           FROM public.consignment_fee_facts
           WHERE contract_id=$1 AND kind='reversed' AND refund_obligation_id=$2"#,
    )
    .bind(command.contract_id)
    .bind(command.refund_obligation_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage)?
    .ok_or(FundingError::Unproven)?;
    Ok(Some(FeeReversal {
        contract_id: command.contract_id,
        receipt_source_id: row.try_get("receipt_source_id").map_err(storage)?,
        refund_obligation_id: command.refund_obligation_id,
        amount: row.try_get("amount").map_err(storage)?,
    }))
}
