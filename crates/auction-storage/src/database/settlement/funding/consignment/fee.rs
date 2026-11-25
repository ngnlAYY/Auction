//! 手续费事实：F>0 时 funded 恰一条 earned、全额退款恰一条 reversed；F=0 全无行（不占位）。
//!
//! 保护不变量 3、8：手续费单独记账，不进补偿池；reversed 只引用本合同那条 earned、同额、至多一次，
//! 只追加不改历史（决定 19）。

use auction_application::settlement::FundingError;
use auction_domain::consignment::ListingQuote;
use auction_domain::consignment::funding::{
    FeeEarned, FeeReversal, FundingRuleError, fee_rows, reverse_fee,
};
use sqlx::{Postgres, Row};
use uuid::Uuid;

use super::contract_allocations::sale_facts;
use crate::database::TrackedTransaction as Transaction;
use crate::database::settlement::funding::{invalid, storage};

// 手续费事实表对运行账号只有 SELECT、INSERT（只追加），无法 FOR UPDATE；并发由调用方持有的 UID 账户锁
// 与 (contract_id, kind) 唯一键串行化。

/// 记录 funded 的 earned：引用本合同的原收据与实际贷入 fee 的那笔分录。
///
/// 返回新写入或已有的 earned 事实 ID；F=0 返回 `None` 且不得已有任何手续费事实。
/// 调用方（收款认证事务，6B2）在 funded 投影插入前、同一事务内调用；调用方已持 UID 账户锁。
#[allow(dead_code, reason = "由 6B2 的 certify_cod 在 funded 投影前调用，集成前尚无调用方")]
pub(crate) async fn write_fee_earned_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    contract_id: Uuid,
    receipt_source_id: Uuid,
    fee_entry_id: Uuid,
    quote: ListingQuote,
) -> Result<Option<Uuid>, FundingError> {
    // 报价必须就是成交时冻结的那一份，否则手续费与合同失配。
    let sale = sale_facts(tx, contract_id).await?;
    if sale.fee != quote.fee || sale.seller_amount != quote.seller_amount {
        return Err(FundingError::Conflict);
    }
    let existing = sqlx::query(
        r#"SELECT id, kind, amount, receipt_source_id, fee_entry_id
           FROM public.consignment_fee_facts
           WHERE contract_id=$1 AND kind='earned'"#,
    )
    .bind(contract_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage)?;
    if !fee_rows(quote).earned {
        return if existing.is_none() { Ok(None) } else { Err(FundingError::Conflict) };
    }
    if let Some(row) = existing {
        let same = row.try_get::<i64, _>("amount").map_err(storage)? == quote.fee
            && row.try_get::<Uuid, _>("receipt_source_id").map_err(storage)? == receipt_source_id
            && row.try_get::<Uuid, _>("fee_entry_id").map_err(storage)? == fee_entry_id;
        return if same {
            row.try_get("id").map(Some).map_err(storage)
        } else {
            Err(FundingError::Conflict)
        };
    }
    let id = Uuid::new_v4();
    sqlx::query(
        r#"INSERT INTO public.consignment_fee_facts
               (id, contract_id, kind, amount, receipt_source_id, fee_entry_id)
           VALUES ($1, $2, 'earned', $3, $4, $5)"#,
    )
    .bind(id)
    .bind(contract_id)
    .bind(quote.fee)
    .bind(receipt_source_id)
    .bind(fee_entry_id)
    .execute(&mut **tx)
    .await
    .map_err(storage)?;
    Ok(Some(id))
}

/// 全额退款转换时冲回本笔手续费：恰一条 reversed，引用同一 earned、同额与实际 fee→refund 分录。
///
/// F=0 返回 `None`（无 earned 也无 reversed）；已冲回过返回冲突。调用方已持 UID 账户锁。
pub(super) async fn write_fee_reversed_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    contract_id: Uuid,
    quote: ListingQuote,
    refund_obligation_id: Uuid,
    fee_to_refund_entry: Option<Uuid>,
) -> Result<Option<FeeReversal>, FundingError> {
    let rows = sqlx::query(
        r#"SELECT id, kind, amount, receipt_source_id
           FROM public.consignment_fee_facts
           WHERE contract_id=$1"#,
    )
    .bind(contract_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(storage)?;
    let mut earned: Option<(Uuid, FeeEarned)> = None;
    let mut already_reversed = false;
    for row in rows {
        let kind: String = row.try_get("kind").map_err(storage)?;
        if kind == "reversed" {
            already_reversed = true;
        } else {
            let id: Uuid = row.try_get("id").map_err(storage)?;
            earned = Some((
                id,
                FeeEarned {
                    contract_id,
                    receipt_source_id: row.try_get("receipt_source_id").map_err(storage)?,
                    amount: row.try_get("amount").map_err(storage)?,
                },
            ));
        }
    }
    let reversal = match reverse_fee(
        contract_id,
        quote,
        earned.map(|(_, fact)| fact),
        already_reversed,
        refund_obligation_id,
    ) {
        Ok(reversal) => reversal,
        Err(FundingRuleError::FeeBinding | FundingRuleError::FeeAlreadyReversed) => {
            return Err(FundingError::Conflict);
        }
        Err(error) => return Err(invalid(error)),
    };
    let (Some(reversal), Some((earned_id, _))) = (reversal, earned) else { return Ok(None) };
    let fee_entry_id = fee_to_refund_entry.ok_or(FundingError::Invalid)?;
    sqlx::query(
        r#"INSERT INTO public.consignment_fee_facts
               (id, contract_id, kind, amount, receipt_source_id, fee_entry_id, earned_fact_id,
                refund_obligation_id)
           VALUES ($1, $2, 'reversed', $3, $4, $5, $6, $7)"#,
    )
    .bind(Uuid::new_v4())
    .bind(contract_id)
    .bind(reversal.amount)
    .bind(reversal.receipt_source_id)
    .bind(fee_entry_id)
    .bind(earned_id)
    .bind(refund_obligation_id)
    .execute(&mut **tx)
    .await
    .map_err(storage)?;
    Ok(Some(reversal))
}
