use super::*;

#[cfg(feature = "postgres")]
pub(in crate::database::account) fn map_postgres_bid(
    row: sqlx::postgres::PgRow,
) -> Result<PublicMyBid, ApplicationError> {
    let bid = PublicMyBid {
        id: get_pg(&row, "id", "读取 PostgreSQL 用户竞价标识失败")?,
        auction_id: get_pg(&row, "auction_id", "读取 PostgreSQL 用户竞价拍卖标识失败")?,
        lot_id: get_pg(&row, "lot_id", "读取 PostgreSQL 用户竞价 lot 标识失败")?,
        lot_number: i64::from(get_pg::<i32>(
            &row,
            "lot_number",
            "读取 PostgreSQL 用户竞价序号失败",
        )?),
        amount: get_pg(&row, "amount", "读取 PostgreSQL 用户竞价金额失败")?,
        accepted_state_version: get_pg(
            &row,
            "accepted_state_version",
            "读取 PostgreSQL 用户竞价状态版本失败",
        )?,
        fact_index: get_pg(&row, "fact_index", "读取 PostgreSQL 用户竞价事实序号失败")?,
        mechanism: get_pg(&row, "mechanism", "读取 PostgreSQL 用户竞价机制失败")?,
        created_at: get_pg(&row, "created_at", "读取 PostgreSQL 用户竞价时间失败")?,
        equipment_name: get_pg(&row, "equipment_name", "读取 PostgreSQL 用户竞价装备名称失败")?,
        equipment_category: get_pg(
            &row,
            "equipment_category",
            "读取 PostgreSQL 用户竞价装备分类失败",
        )?,
        leading: get_pg(&row, "leading", "读取 PostgreSQL 用户竞价领先状态失败")?,
    };
    validate_my_bid(bid)
}

#[cfg(feature = "postgres")]
pub(in crate::database::account) fn map_postgres_proxy_bid(
    row: sqlx::postgres::PgRow,
) -> Result<PublicProxyBid, ApplicationError> {
    let bid = PublicProxyBid {
        id: get_pg(&row, "id", "读取 PostgreSQL 代理竞价标识失败")?,
        auction_id: get_pg(&row, "auction_id", "读取 PostgreSQL 代理竞价拍卖标识失败")?,
        lot_id: get_pg(&row, "lot_id", "读取 PostgreSQL 代理竞价 lot 标识失败")?,
        lot_number: i64::from(get_pg::<i32>(
            &row,
            "lot_number",
            "读取 PostgreSQL 代理竞价序号失败",
        )?),
        amount: get_pg(&row, "amount", "读取 PostgreSQL 代理竞价金额失败")?,
        accepted_state_version: get_pg(
            &row,
            "accepted_state_version",
            "读取 PostgreSQL 代理竞价状态版本失败",
        )?,
        fact_index: get_pg(&row, "fact_index", "读取 PostgreSQL 代理竞价事实序号失败")?,
        mechanism: get_pg(&row, "mechanism", "读取 PostgreSQL 代理竞价机制失败")?,
        created_at: get_pg(&row, "created_at", "读取 PostgreSQL 代理竞价时间失败")?,
        leading: get_pg(&row, "leading", "读取 PostgreSQL 代理竞价领先状态失败")?,
    };
    validate_proxy_bid(bid)
}

pub(in crate::database::account) fn validate_my_bid(
    bid: PublicMyBid,
) -> Result<PublicMyBid, ApplicationError> {
    if bid.id.is_nil()
        || bid.auction_id.is_nil()
        || bid.lot_id.is_nil()
        || bid.lot_number <= 0
        || bid.amount <= 0
        || bid.accepted_state_version <= 0
        || bid.fact_index <= 0
        || (bid.mechanism != "ordinary" && bid.mechanism != "proxy")
        || bid.created_at.timestamp_micros() == 0
        || bid.equipment_name.trim().is_empty()
        || bid.equipment_category.trim().is_empty()
    {
        return Err(storage_error("账户竞价投影包含无效事实"));
    }
    Ok(bid)
}

pub(in crate::database::account) fn validate_proxy_bid(
    bid: PublicProxyBid,
) -> Result<PublicProxyBid, ApplicationError> {
    if bid.id.is_nil()
        || bid.auction_id.is_nil()
        || bid.lot_id.is_nil()
        || bid.lot_number <= 0
        || bid.amount <= 0
        || bid.accepted_state_version <= 0
        || bid.fact_index <= 0
        || bid.mechanism != "proxy"
        || bid.created_at.timestamp_micros() == 0
    {
        return Err(storage_error("账户代理竞价投影包含无效事实"));
    }
    Ok(bid)
}
