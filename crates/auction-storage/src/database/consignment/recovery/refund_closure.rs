//! 退款转换的共同部分：按锁内 Sale 事实构造 6B 单写者 `convert_refund_in_tx` 的命令，转换后追加
//! Sale 的 `refund_closed` 封闭事实。Admin 退款类处置（7E-2）与退回处置的 `refund_p` 效果（7A-2）共用。
//!
//! 保护不变量 1、4、6、8：资金只由 6B 写者改动；金额只取 Sale 的价格与费率并经领域构造，不信任调用方；
//! 封闭事实只追加，重复写入无副作用。

use auction_application::settlement::{ConvertRefund, FundingError, FundingMutation};
use auction_domain::consignment::{FeeBps, ListingPrice, quote_v1};
use sqlx::{PgConnection, Postgres};
use uuid::Uuid;

use crate::database::TrackedTransaction;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::settlement::funding::storage;

/// 由 Sale 的价格、费率与买家退款义务构造转换命令；不是 Sale 或缺买家退款义务时为 `None`。
pub(crate) async fn refund_command(
    connection: &mut PgConnection,
    contract_id: Uuid,
    mutation: FundingMutation,
) -> Result<Option<ConvertRefund>, WriteError> {
    let sale: Option<(i64, i16, Uuid)> = sqlx::query_as(
        "SELECT sale.price, sale.fee_bps, obligation.id
         FROM public.consignment_sales AS sale
         JOIN public.settlement_obligations AS obligation
           ON obligation.contract_id = sale.contract_id AND obligation.obligation_kind = 'buyer_refund'
         WHERE sale.contract_id = $1",
    )
    .bind(contract_id)
    .fetch_optional(&mut *connection)
    .await?;
    let Some((price, fee_bps, refund_obligation_id)) = sale else { return Ok(None) };
    // 价格与费率列由 c5_sale_* 约束保证；读出后仍经领域构造，损坏时失败关闭。
    let price = ListingPrice::new(price).map_err(|error| {
        tracing::warn!(%error, "寄售成交价格无效");
        WriteError::invariant("consignment_sale_price")
    })?;
    let fee_bps = FeeBps::new(i64::from(fee_bps)).map_err(|error| {
        tracing::warn!(%error, "寄售成交费率无效");
        WriteError::invariant("consignment_sale_fee")
    })?;
    Ok(Some(ConvertRefund {
        mutation,
        contract_id,
        refund_obligation_id,
        price,
        quote: quote_v1(price, fee_bps),
    }))
}

/// 转换成功后的 Sale 封闭事实；`basis_ref` 指向 6B 资金命令。
pub(crate) async fn record_refund_closure(
    tx: &mut TrackedTransaction<'_, Postgres>,
    contract_id: Uuid,
    basis_ref: &str,
) -> Result<(), FundingError> {
    sqlx::query(
        "INSERT INTO public.consignment_sale_closures
            (contract_id, closure_kind, basis_ref, closed_at)
         VALUES ($1, 'refund_closed', $2, clock_timestamp())
         ON CONFLICT ON CONSTRAINT c7_closure_pk DO NOTHING",
    )
    .bind(contract_id)
    .bind(basis_ref)
    .execute(&mut **tx)
    .await
    .map_err(storage)?;
    Ok(())
}
