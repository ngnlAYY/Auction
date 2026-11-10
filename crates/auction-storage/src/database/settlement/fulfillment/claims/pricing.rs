//! 履约领取时投影不可变结算明细；正文不读取现行策略或可变竞价状态。

use auction_application::core::RepositoryError;
use auction_domain::{LotId, SettlementAmount, SettlementLine, SettlementMode};
use sqlx::{Row, postgres::PgRow};
use uuid::Uuid;

use super::{map_storage_error, repository_error_owned};

pub(super) fn map_settlement_line(row: &PgRow) -> Result<Option<SettlementLine>, RepositoryError> {
    let Some(lot_id) =
        row.try_get::<Option<Uuid>, _>("settlement_lot_id").map_err(map_storage_error)?
    else {
        return Ok(None);
    };
    let amount = |column: &str| -> Result<SettlementAmount, RepositoryError> {
        SettlementAmount::new(row.try_get(column).map_err(map_storage_error)?)
            .map_err(|_| repository_error_owned("履约邮件结算金额无效"))
    };
    let line = SettlementLine {
        lot_id: LotId::from_uuid(lot_id),
        end_price: amount("end_price")?,
        listed_price: amount("listed_price")?,
        buyer_amount: amount("buyer_amount")?,
        seller_amount: amount("seller_amount")?,
        tax_amount: amount("tax_amount")?,
        compensation_amount: amount("compensation_amount")?,
        hold_ppm: row.try_get("hold_ppm").map_err(map_storage_error)?,
        adjustment_ppm: row.try_get("adjustment_ppm").map_err(map_storage_error)?,
    };
    let mode = map_settlement_mode(row)?
        .ok_or_else(|| repository_error_owned("履约邮件缺少冻结结算模式"))?;
    line.validate_for_mode(mode).map_err(|_| repository_error_owned("履约邮件冻结结算明细无效"))?;
    Ok(Some(line))
}

/// 仅缺少历史 policy 标签时沿用 v1，未知标签失败关闭；不读取运行配置。
pub(super) fn map_settlement_mode(row: &PgRow) -> Result<Option<SettlementMode>, RepositoryError> {
    if row.try_get::<Option<Uuid>, _>("settlement_lot_id").map_err(map_storage_error)?.is_none() {
        return Ok(None);
    }
    let mode: Option<String> = row.try_get("settlement_policy_mode").map_err(map_storage_error)?;
    crate::database::settlement::policy::decode_mode(mode.as_deref().unwrap_or("hold_curve_v1"))
        .map(Some)
        .map_err(repository_error_owned)
}
