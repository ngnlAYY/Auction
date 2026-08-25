use super::*;

#[cfg(feature = "postgres")]
pub(in crate::database::account) fn map_postgres_proxy_base(
    row: sqlx::postgres::PgRow,
) -> Result<ProxyBase, ApplicationError> {
    Ok(ProxyBase {
        authorization_id: get_pg(&row, "authorization_id", "读取 PostgreSQL 代理授权标识失败")?,
        auction_id: get_pg(&row, "auction_id", "读取 PostgreSQL 代理拍卖标识失败")?,
        lot_id: get_pg(&row, "lot_id", "读取 PostgreSQL 代理 lot 标识失败")?,
        user_id: get_pg(&row, "user_id", "读取 PostgreSQL 代理用户标识失败")?,
        lot_number: i64::from(get_pg::<i32>(
            &row,
            "lot_number",
            "读取 PostgreSQL 代理 lot 序号失败",
        )?),
        authorization_sequence: get_pg(
            &row,
            "authorization_sequence",
            "读取 PostgreSQL 代理授权序号失败",
        )?,
        usage_number: get_pg(&row, "usage_number", "读取 PostgreSQL 代理使用序号失败")?,
        quota_used: get_pg(&row, "quota_used", "读取 PostgreSQL 代理 quota 使用量失败")?,
        quota_limit: get_pg(&row, "quota_limit", "读取 PostgreSQL 代理 quota 上限失败")?,
        mode: get_pg(&row, "mode", "读取 PostgreSQL 代理模式失败")?,
        maximum_amount: get_pg(&row, "maximum_amount", "读取 PostgreSQL 代理金额失败")?,
        status: get_pg(&row, "proxy_status", "读取 PostgreSQL 代理状态失败")?,
        terminal_reason: get_pg(&row, "terminal_reason", "读取 PostgreSQL 代理终止原因失败")?,
        state_version: get_pg(&row, "state_version", "读取 PostgreSQL 代理状态版本失败")?,
        created_at: get_pg(&row, "created_at", "读取 PostgreSQL 代理创建时间失败")?,
        updated_at: get_pg(&row, "updated_at", "读取 PostgreSQL 代理更新时间失败")?,
        equipment_name: get_pg(&row, "equipment_name", "读取 PostgreSQL 代理装备名称失败")?,
        equipment_category: get_pg(&row, "equipment_category", "读取 PostgreSQL 代理装备分类失败")?,
        start_price: get_pg(&row, "start_price", "读取 PostgreSQL 代理起始价失败")?,
        current_price: get_pg(&row, "current_price", "读取 PostgreSQL 代理当前价失败")?,
        material: get_pg(&row, "material", "读取 PostgreSQL 代理 material 标志失败")?,
        leader_user_id: get_pg(&row, "leader_user_id", "读取 PostgreSQL 代理领先用户失败")?,
        slow_authorization_id: get_pg(
            &row,
            "slow_authorization_id",
            "读取 PostgreSQL 代理等待授权失败",
        )?,
    })
}

#[cfg(feature = "sqlite")]
pub(in crate::database::account) fn map_sqlite_proxy_base(
    row: sqlx::sqlite::SqliteRow,
) -> Result<ProxyBase, ApplicationError> {
    Ok(ProxyBase {
        authorization_id: get_sqlite_uuid(
            &row,
            "authorization_id",
            "读取 SQLite 代理授权标识失败",
        )?,
        auction_id: get_sqlite_uuid(&row, "auction_id", "读取 SQLite 代理拍卖标识失败")?,
        lot_id: get_sqlite_uuid(&row, "lot_id", "读取 SQLite 代理 lot 标识失败")?,
        user_id: get_sqlite_uuid(&row, "user_id", "读取 SQLite 代理用户标识失败")?,
        lot_number: get_sqlite(&row, "lot_number", "读取 SQLite 代理 lot 序号失败")?,
        authorization_sequence: get_sqlite(
            &row,
            "authorization_sequence",
            "读取 SQLite 代理授权序号失败",
        )?,
        usage_number: get_sqlite(&row, "usage_number", "读取 SQLite 代理使用序号失败")?,
        quota_used: get_sqlite(&row, "quota_used", "读取 SQLite 代理 quota 使用量失败")?,
        quota_limit: get_sqlite(&row, "quota_limit", "读取 SQLite 代理 quota 上限失败")?,
        mode: get_sqlite(&row, "mode", "读取 SQLite 代理模式失败")?,
        maximum_amount: get_sqlite(&row, "maximum_amount", "读取 SQLite 代理金额失败")?,
        status: get_sqlite(&row, "proxy_status", "读取 SQLite 代理状态失败")?,
        terminal_reason: get_sqlite(&row, "terminal_reason", "读取 SQLite 代理终止原因失败")?,
        state_version: get_sqlite(&row, "state_version", "读取 SQLite 代理状态版本失败")?,
        created_at: parse_timestamp(
            get_sqlite(&row, "created_at", "读取 SQLite 代理创建时间失败")?,
            "SQLite 代理创建时间格式无效",
        )?,
        updated_at: parse_timestamp(
            get_sqlite(&row, "updated_at", "读取 SQLite 代理更新时间失败")?,
            "SQLite 代理更新时间格式无效",
        )?,
        equipment_name: get_sqlite(&row, "equipment_name", "读取 SQLite 代理装备名称失败")?,
        equipment_category: get_sqlite(&row, "equipment_category", "读取 SQLite 代理装备分类失败")?,
        start_price: get_sqlite(&row, "start_price", "读取 SQLite 代理起始价失败")?,
        current_price: get_sqlite(&row, "current_price", "读取 SQLite 代理当前价失败")?,
        material: get_sqlite::<i64>(&row, "material", "读取 SQLite 代理 material 标志失败")? != 0,
        leader_user_id: get_sqlite_optional_uuid(
            &row,
            "leader_user_id",
            "读取 SQLite 代理领先用户失败",
        )?,
        slow_authorization_id: get_sqlite_optional_uuid(
            &row,
            "slow_authorization_id",
            "读取 SQLite 代理等待授权失败",
        )?,
    })
}

#[cfg(feature = "postgres")]
pub(in crate::database::account) fn map_postgres_transition(
    row: sqlx::postgres::PgRow,
) -> Result<PublicProxyTransition, ApplicationError> {
    let transition = PublicProxyTransition {
        transition_number: get_pg(&row, "transition_number", "读取 PostgreSQL 代理转换序号失败")?,
        kind: get_pg(&row, "kind", "读取 PostgreSQL 代理转换类型失败")?,
        old_maximum_amount: get_pg(&row, "old_maximum_amount", "读取 PostgreSQL 代理旧上限失败")?,
        new_maximum_amount: get_pg(&row, "new_maximum_amount", "读取 PostgreSQL 代理新上限失败")?,
        old_mode: get_pg(&row, "old_mode", "读取 PostgreSQL 代理旧模式失败")?,
        new_mode: get_pg(&row, "new_mode", "读取 PostgreSQL 代理新模式失败")?,
        old_status: get_pg(&row, "old_status", "读取 PostgreSQL 代理旧状态失败")?,
        new_status: get_pg(&row, "new_status", "读取 PostgreSQL 代理新状态失败")?,
        reason: get_pg(&row, "reason", "读取 PostgreSQL 代理转换原因失败")?,
        resulting_state_version: get_pg(
            &row,
            "resulting_state_version",
            "读取 PostgreSQL 代理转换状态版本失败",
        )?,
        created_at: get_pg(&row, "created_at", "读取 PostgreSQL 代理转换时间失败")?,
    };
    normalize_transition(transition)
}

#[cfg(feature = "sqlite")]
pub(in crate::database::account) fn map_sqlite_transition(
    row: sqlx::sqlite::SqliteRow,
) -> Result<PublicProxyTransition, ApplicationError> {
    let transition = PublicProxyTransition {
        transition_number: get_sqlite(&row, "transition_number", "读取 SQLite 代理转换序号失败")?,
        kind: get_sqlite(&row, "kind", "读取 SQLite 代理转换类型失败")?,
        old_maximum_amount: get_sqlite(&row, "old_maximum_amount", "读取 SQLite 代理旧上限失败")?,
        new_maximum_amount: get_sqlite(&row, "new_maximum_amount", "读取 SQLite 代理新上限失败")?,
        old_mode: get_sqlite(&row, "old_mode", "读取 SQLite 代理旧模式失败")?,
        new_mode: get_sqlite(&row, "new_mode", "读取 SQLite 代理新模式失败")?,
        old_status: get_sqlite(&row, "old_status", "读取 SQLite 代理旧状态失败")?,
        new_status: get_sqlite(&row, "new_status", "读取 SQLite 代理新状态失败")?,
        reason: get_sqlite(&row, "reason", "读取 SQLite 代理转换原因失败")?,
        resulting_state_version: get_sqlite(
            &row,
            "resulting_state_version",
            "读取 SQLite 代理转换状态版本失败",
        )?,
        created_at: parse_timestamp(
            get_sqlite(&row, "created_at", "读取 SQLite 代理转换时间失败")?,
            "SQLite 代理转换时间格式无效",
        )?,
    };
    normalize_transition(transition)
}

pub(in crate::database::account) fn normalize_transition(
    mut transition: PublicProxyTransition,
) -> Result<PublicProxyTransition, ApplicationError> {
    if transition.transition_number <= 0
        || transition.new_maximum_amount <= 0
        || transition.resulting_state_version <= 0
        || transition.kind.trim().is_empty()
        || transition.new_mode.trim().is_empty()
        || transition.new_status.trim().is_empty()
        || transition.created_at.timestamp_micros() == 0
    {
        return Err(storage_error("代理转换历史包含无效字段"));
    }
    transition.new_status = public_status(&transition.new_status)?;
    if let Some(status) = transition.old_status.as_mut() {
        *status = public_status(status)?;
    }
    Ok(transition)
}

pub(in crate::database::account) fn build_proxy(
    base: ProxyBase,
    transitions: Vec<PublicProxyTransition>,
    bid: Option<PublicProxyBid>,
) -> Result<PublicOwnedProxy, ApplicationError> {
    let transition_count =
        i64::try_from(transitions.len()).map_err(|_| storage_error("代理转换数量溢出"))?;
    if base.authorization_id.is_nil()
        || base.auction_id.is_nil()
        || base.lot_id.is_nil()
        || base.user_id.is_nil()
        || base.lot_number <= 0
        || base.authorization_sequence <= 0
        || base.usage_number <= 0
        || base.quota_used <= 0
        || base.quota_limit < 0
        || base.quota_used < base.usage_number
        || base.quota_limit > 0 && base.quota_used > base.quota_limit
        || base.maximum_amount <= 0
        || base.start_price <= 0
        || base.state_version <= 0
        || base.updated_at < base.created_at
        || base.equipment_name.trim().is_empty()
        || base.equipment_category.trim().is_empty()
        || transition_count != base.state_version
    {
        return Err(storage_error("账户代理投影包含无效快照"));
    }
    for (index, transition) in transitions.iter().enumerate() {
        let expected = i64::try_from(index)
            .ok()
            .and_then(|index| index.checked_add(1))
            .ok_or_else(|| storage_error("代理转换序号溢出"))?;
        if transition.transition_number != expected
            || transition.resulting_state_version != expected
        {
            return Err(storage_error("账户代理转换历史版本不连续"));
        }
    }
    if base.mode != "fast" && base.mode != "slow" {
        return Err(storage_error("账户代理模式不在公开闭集内"));
    }
    let public_status = public_status(&base.status)?;
    let active = base.status == "active" || base.status == "final_fast";
    let invalid_reason = match public_status.as_str() {
        "exhausted" => Some("outbid".to_owned()),
        "frozen" => Some("frozen".to_owned()),
        "closed" => Some("closed".to_owned()),
        "active" => None,
        _ => return Err(storage_error("账户代理状态映射失败")),
    };
    let capabilities = if !active {
        PublicProxyCapabilities { can_lower: false, waiting: false, at_maximum: false }
    } else {
        derive_capabilities(&base)?
    };
    if let Some(bid) = &bid
        && (bid.auction_id != base.auction_id || bid.lot_id != base.lot_id)
    {
        return Err(storage_error("账户代理最近竞价与 lot 不匹配"));
    }
    Ok(PublicOwnedProxy {
        authorization_id: base.authorization_id,
        auction_id: base.auction_id,
        lot_id: base.lot_id,
        user_id: base.user_id,
        lot_number: base.lot_number,
        authorization_sequence: base.authorization_sequence,
        usage_number: base.usage_number,
        quota_used: base.quota_used,
        quota_limit: base.quota_limit,
        mode: base.mode,
        maximum_amount: base.maximum_amount,
        status: public_status,
        terminal_reason: base.terminal_reason,
        state_version: base.state_version,
        transitions,
        created_at: base.created_at,
        updated_at: base.updated_at,
        equipment_name: base.equipment_name,
        equipment_category: base.equipment_category,
        bid,
        invalid_reason,
        capabilities,
    })
}

pub(in crate::database::account) fn derive_capabilities(
    base: &ProxyBase,
) -> Result<PublicProxyCapabilities, ApplicationError> {
    let leading = base.leader_user_id == Some(base.user_id);
    let waiting = base.slow_authorization_id == Some(base.authorization_id);
    if leading == waiting
        || leading && base.slow_authorization_id.is_some()
        || waiting && (base.mode != "slow" || base.leader_user_id.is_none())
    {
        return Err(storage_error("账户代理领先/等待状态不一致"));
    }
    let current_price = base.current_price.unwrap_or(base.start_price);
    let current =
        BidAmount::new(current_price).map_err(|_| storage_error("账户代理当前金额无效"))?;
    let start =
        BidAmount::new(base.start_price).map_err(|_| storage_error("账户代理起始金额无效"))?;
    let binding = if waiting {
        AuctionRules::default()
            .next_minimum_bid(Some(start), Some(current), base.material, false)
            .map_err(|_| storage_error("账户代理绑定金额超出可表示范围"))?
    } else {
        current
    };
    if base.maximum_amount < binding.value() {
        return Err(storage_error("账户代理上限低于当前合法绑定金额"));
    }
    Ok(PublicProxyCapabilities {
        can_lower: base.maximum_amount > binding.value(),
        waiting,
        at_maximum: base.maximum_amount == current.value(),
    })
}

pub(in crate::database::account) fn public_status(
    status: &str,
) -> Result<String, ApplicationError> {
    match status {
        "active" | "final_fast" => Ok("active".to_owned()),
        "exhausted" => Ok("exhausted".to_owned()),
        "frozen" => Ok("frozen".to_owned()),
        "closed" => Ok("closed".to_owned()),
        _ => Err(storage_error("账户代理状态未知")),
    }
}
