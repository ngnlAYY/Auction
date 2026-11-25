//! 寄售合同键下的资金分配与成交冻结事实读取。
//!
//! 分配唯一键是 `(uid, purpose, lot_id, consignment_contract_id)`；寄售只带合同、不带 lot，
//! 插入、冲突与锁读使用同一键（D9 §4.3）。拍卖的 `ledger::allocation` 只认合同为空的行，不得复用。

use auction_application::settlement::FundingError;
use auction_domain::consignment::funding::FundingMode;
use sqlx::{Postgres, Row};
use uuid::Uuid;

use crate::database::TrackedTransaction as Transaction;
use crate::database::settlement::funding::storage;

/// 成交时冻结的资金身份与金额：UID、模式与 P / F / S。
#[derive(Debug, Clone)]
pub(super) struct SaleFacts {
    pub(super) funding_uid: String,
    pub(super) funding_mode: FundingMode,
    pub(super) price: i64,
    pub(super) fee: i64,
    pub(super) seller_amount: i64,
}

/// 读取合同的冻结事实；没有 Sale 行说明合同不是成交或不存在，失败关闭。
pub(super) async fn sale_facts(
    tx: &mut Transaction<'_, Postgres>,
    contract_id: Uuid,
) -> Result<SaleFacts, FundingError> {
    let row = sqlx::query(
        r#"SELECT funding_uid, funding_mode, price, fee, seller_amount
           FROM public.consignment_sales
           WHERE contract_id=$1"#,
    )
    .bind(contract_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage)?
    .ok_or(FundingError::Unproven)?;
    let mode: String = row.try_get("funding_mode").map_err(storage)?;
    Ok(SaleFacts {
        funding_uid: row.try_get("funding_uid").map_err(storage)?,
        funding_mode: parse_mode(&mode)?,
        price: row.try_get("price").map_err(storage)?,
        fee: row.try_get("fee").map_err(storage)?,
        seller_amount: row.try_get("seller_amount").map_err(storage)?,
    })
}

/// 持久模式码；未知值失败关闭。
pub(super) fn parse_mode(value: &str) -> Result<FundingMode, FundingError> {
    match value {
        "strict_cash" => Ok(FundingMode::StrictCash),
        "record_only" => Ok(FundingMode::RecordOnly),
        _ => Err(FundingError::Unproven),
    }
}

/// 模式的持久码。
pub(super) const fn mode_code(mode: FundingMode) -> &'static str {
    match mode {
        FundingMode::StrictCash => "strict_cash",
        FundingMode::RecordOnly => "record_only",
    }
}

/// 一行已锁定的合同分配。
#[derive(Debug, Clone)]
pub(super) struct AllocationRow {
    pub(super) id: Uuid,
    pub(super) amount: i64,
    pub(super) state: String,
    pub(super) delivery_id: Option<Uuid>,
}

/// 锁读合同分配；不存在返回 `None`。调用方已持 UID 账户锁。
pub(super) async fn locked_allocation(
    tx: &mut Transaction<'_, Postgres>,
    uid: &str,
    purpose: &str,
    contract_id: Uuid,
) -> Result<Option<AllocationRow>, FundingError> {
    let row = sqlx::query(
        r#"SELECT id, amount, state, delivery_id
           FROM public.settlement_funding_allocations
           WHERE uid=$1 AND purpose=$2 AND lot_id IS NULL AND consignment_contract_id=$3
           FOR UPDATE"#,
    )
    .bind(uid)
    .bind(purpose)
    .bind(contract_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage)?;
    row.map(|row| {
        Ok(AllocationRow {
            id: row.try_get("id").map_err(storage)?,
            amount: row.try_get("amount").map_err(storage)?,
            state: row.try_get("state").map_err(storage)?,
            delivery_id: row.try_get("delivery_id").map_err(storage)?,
        })
    })
    .transpose()
}

/// 取得（必要时零额建立）合同分配并锁定；`obligation` 给出时与已有绑定必须一致，已有行未绑定则补绑。
pub(super) async fn contract_allocation(
    tx: &mut Transaction<'_, Postgres>,
    uid: &str,
    purpose: &str,
    contract_id: Uuid,
    obligation: Option<Uuid>,
) -> Result<Uuid, FundingError> {
    sqlx::query(
        r#"INSERT INTO public.settlement_funding_allocations
               (id, uid, purpose, lot_id, obligation_id, amount, consignment_contract_id)
           VALUES ($1, $2, $3, NULL, $4, 0, $5)
           ON CONFLICT (uid, purpose, lot_id, consignment_contract_id) DO NOTHING"#,
    )
    .bind(Uuid::new_v4())
    .bind(uid)
    .bind(purpose)
    .bind(obligation)
    .bind(contract_id)
    .execute(&mut **tx)
    .await
    .map_err(storage)?;
    let row = sqlx::query(
        r#"SELECT id, obligation_id
           FROM public.settlement_funding_allocations
           WHERE uid=$1 AND purpose=$2 AND lot_id IS NULL AND consignment_contract_id=$3
           FOR UPDATE"#,
    )
    .bind(uid)
    .bind(purpose)
    .bind(contract_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(storage)?;
    let id: Uuid = row.try_get("id").map_err(storage)?;
    let bound: Option<Uuid> = row.try_get("obligation_id").map_err(storage)?;
    match (bound, obligation) {
        (Some(bound), Some(wanted)) if bound != wanted => Err(FundingError::Conflict),
        (None, Some(wanted)) => {
            sqlx::query(
                "UPDATE public.settlement_funding_allocations SET obligation_id=$1 WHERE id=$2",
            )
            .bind(wanted)
            .bind(id)
            .execute(&mut **tx)
            .await
            .map_err(storage)?;
            Ok(id)
        }
        _ => Ok(id),
    }
}
