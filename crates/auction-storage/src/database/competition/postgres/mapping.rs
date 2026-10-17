//! PostgreSQL 竞价事务的共享映射、校验和错误转换；事务边界由父模块统一装配。

use super::*;

pub(crate) fn bid_mechanism_name(mechanism: BidMechanism) -> &'static str {
    match mechanism {
        BidMechanism::Ordinary => "ordinary",
        BidMechanism::Proxy => "proxy",
    }
}

pub(crate) fn plan_command_id(
    plan: &CompetitionPlan,
) -> Result<auction_domain::CommandId, CompetitionTransactionError> {
    plan.public_bids
        .first()
        .map(|bid| bid.command_id)
        .or_else(|| plan.proxy_transitions.first().map(|transition| transition.command_id))
        .ok_or_else(|| invalid_state("竞价计划没有可审计的命令标识"))
}

pub(crate) fn terminal_reason(
    plan: &CompetitionPlan,
    authorization_id: ProxyAuthorizationId,
) -> Option<ProxyTerminalReason> {
    plan.proxy_transitions
        .iter()
        .rev()
        .find(|transition| transition.authorization_id == authorization_id)
        .and_then(|transition| transition.reason)
}

pub(crate) fn proxy_from_row(
    row: sqlx::postgres::PgRow,
) -> Result<ProxyAuthorization, CompetitionTransactionError> {
    let mode = parse_proxy_mode(row.try_get::<String, _>("mode").map_err(map_sql_error)?)?;
    let status = parse_proxy_status(row.try_get::<String, _>("status").map_err(map_sql_error)?)?;
    let authorization = ProxyAuthorization {
        id: ProxyAuthorizationId::from_uuid(
            row.try_get("authorization_id").map_err(map_sql_error)?,
        ),
        lot_id: LotId::from_uuid(row.try_get("lot_id").map_err(map_sql_error)?),
        user_id: auction_domain::UserId::from_uuid(row.try_get("user_id").map_err(map_sql_error)?),
        creation_command_id: auction_domain::CommandId::from_uuid(
            row.try_get("creation_command_id").map_err(map_sql_error)?,
        ),
        authorization_sequence: positive_u64(
            row.try_get("authorization_sequence").map_err(map_sql_error)?,
            "代理序号",
        )?,
        usage_number: positive_u64(
            row.try_get("usage_number").map_err(map_sql_error)?,
            "代理使用次数",
        )?,
        maximum_amount: bid_amount(&row, "maximum_amount", "代理上限")?,
        mode,
        status,
        state_version: positive_u64(
            row.try_get("state_version").map_err(map_sql_error)?,
            "代理状态版本",
        )?,
    };
    authorization.validate().map_err(|error| invalid_state(error.to_string().as_str()))?;
    Ok(authorization)
}

pub(crate) fn valid_proxy_terminal_reason(status: ProxyStatus, reason: Option<&str>) -> bool {
    match status {
        ProxyStatus::Active | ProxyStatus::FinalFast => reason.is_none(),
        ProxyStatus::Exhausted => matches!(reason, Some("strictly_defeated" | "ranked_below")),
        ProxyStatus::Frozen => reason == Some("eligibility_lost"),
        ProxyStatus::Closed => reason == Some("competition_closed"),
    }
}

pub(crate) fn bid_amount(
    row: &sqlx::postgres::PgRow,
    column: &str,
    label: &str,
) -> Result<BidAmount, CompetitionTransactionError> {
    let value: i64 = row.try_get(column).map_err(map_sql_error)?;
    BidAmount::new(value).map_err(|_| invalid_state(label))
}

pub(crate) fn auction_rules_from_row(
    row: &PgRow,
) -> Result<AuctionRules, CompetitionTransactionError> {
    let rules = AuctionRules {
        default_start_price: bid_amount(row, "default_start_price", "拍卖默认起始价")?,
        bid_increment_ppm: row.try_get("policy_bid_increment_ppm").map_err(map_sql_error)?,
        minimum_increment: row.try_get("policy_minimum_increment").map_err(map_sql_error)?,
        soft_close_window: duration(
            row.try_get("soft_close_window_seconds").map_err(map_sql_error)?,
            "软截止窗口",
        )?,
        soft_close_extension: duration(
            row.try_get("soft_close_extension_seconds").map_err(map_sql_error)?,
            "软截止延长",
        )?,
        cancel_cutoff: duration(
            row.try_get("cancel_cutoff_seconds").map_err(map_sql_error)?,
            "取消截止",
        )?,
        proxy_slow_interval: duration(
            row.try_get("proxy_slow_interval_seconds").map_err(map_sql_error)?,
            "慢速代理间隔",
        )?,
        proxy_enabled: row.try_get("policy_proxy_enabled").map_err(map_sql_error)?,
        proxy_default_mode: parse_proxy_mode(
            row.try_get("policy_proxy_default_mode").map_err(map_sql_error)?,
        )?,
        proxy_max_outbid_uses: policy_u32(
            row.try_get("policy_proxy_max_outbid_uses").map_err(map_sql_error)?,
            "代理重复进场次数",
        )?,
        final_phase_lead: duration(
            row.try_get("policy_final_phase_lead_seconds").map_err(map_sql_error)?,
            "最终阶段领先时间",
        )?,
    };
    rules.validate().map_err(|error| invalid_state(error.to_string().as_str()))?;
    Ok(rules)
}

pub(crate) fn duration(seconds: i64, label: &str) -> Result<Duration, CompetitionTransactionError> {
    Duration::try_seconds(seconds).ok_or_else(|| invalid_state(label))
}

pub(crate) fn positive_u64(value: i64, label: &str) -> Result<u64, CompetitionTransactionError> {
    if value <= 0 {
        return Err(invalid_state(label));
    }
    value.try_into().map_err(|_| invalid_state(label))
}

pub(crate) fn i64_version(value: u64, label: &str) -> Result<i64, CompetitionTransactionError> {
    value.try_into().map_err(|_| invalid_state(label))
}

pub(crate) fn i32_version(value: u32, label: &str) -> Result<i32, CompetitionTransactionError> {
    value.try_into().map_err(|_| invalid_state(label))
}

pub(crate) fn policy_u32(value: i64, label: &str) -> Result<u32, CompetitionTransactionError> {
    if value < 0 {
        return Err(invalid_state(label));
    }
    value.try_into().map_err(|_| invalid_state(label))
}

pub(crate) fn parse_proxy_mode(value: String) -> Result<ProxyMode, CompetitionTransactionError> {
    match value.as_str() {
        "fast" => Ok(ProxyMode::Fast),
        "slow" => Ok(ProxyMode::Slow),
        _ => Err(invalid_state("数据库代理模式无效")),
    }
}

pub(crate) fn parse_proxy_status(
    value: String,
) -> Result<ProxyStatus, CompetitionTransactionError> {
    match value.as_str() {
        "active" => Ok(ProxyStatus::Active),
        "final_fast" => Ok(ProxyStatus::FinalFast),
        "exhausted" => Ok(ProxyStatus::Exhausted),
        "frozen" => Ok(ProxyStatus::Frozen),
        "closed" => Ok(ProxyStatus::Closed),
        _ => Err(invalid_state("数据库代理状态无效")),
    }
}

pub(crate) fn proxy_mode(mode: ProxyMode) -> &'static str {
    match mode {
        ProxyMode::Fast => "fast",
        ProxyMode::Slow => "slow",
    }
}

pub(crate) fn proxy_status(status: ProxyStatus) -> &'static str {
    match status {
        ProxyStatus::Active => "active",
        ProxyStatus::FinalFast => "final_fast",
        ProxyStatus::Exhausted => "exhausted",
        ProxyStatus::Frozen => "frozen",
        ProxyStatus::Closed => "closed",
    }
}

pub(crate) fn proxy_terminal_reason(reason: ProxyTerminalReason) -> &'static str {
    match reason {
        ProxyTerminalReason::StrictlyDefeated => "strictly_defeated",
        ProxyTerminalReason::RankedBelow => "ranked_below",
        ProxyTerminalReason::EligibilityLost => "eligibility_lost",
        ProxyTerminalReason::CompetitionClosed => "competition_closed",
    }
}

pub(crate) fn proxy_transition_kind(kind: auction_domain::ProxyTransitionKind) -> &'static str {
    match kind {
        auction_domain::ProxyTransitionKind::Created => "created",
        auction_domain::ProxyTransitionKind::Lowered => "lowered",
        auction_domain::ProxyTransitionKind::FinalFast => "final_fast",
        auction_domain::ProxyTransitionKind::Exhausted => "exhausted",
        auction_domain::ProxyTransitionKind::Frozen => "frozen",
        auction_domain::ProxyTransitionKind::Closed => "closed",
    }
}

pub(crate) fn bid_mechanism(mechanism: BidMechanism) -> &'static str {
    match mechanism {
        BidMechanism::Ordinary => "ordinary",
        BidMechanism::Proxy => "proxy",
    }
}

pub(crate) async fn set_isolation(
    transaction: Result<&mut PgTransaction, CompetitionTransactionError>,
    isolation: TransactionIsolation,
) -> Result<(), CompetitionTransactionError> {
    let transaction = transaction?;
    let statement = match isolation {
        TransactionIsolation::Serializable => "SET TRANSACTION ISOLATION LEVEL SERIALIZABLE",
        TransactionIsolation::ReadCommitted => "SET TRANSACTION ISOLATION LEVEL READ COMMITTED",
    };
    sqlx::query(statement).execute(&mut **transaction).await.map_err(map_sql_error)?;
    Ok(())
}

pub(crate) fn map_sql_error(error: sqlx::Error) -> CompetitionTransactionError {
    if is_serialization_error(&error) {
        CompetitionTransactionError::SerializationConflict
    } else if is_unique_error(&error) {
        CompetitionTransactionError::Conflict
    } else {
        CompetitionTransactionError::Failed("PostgreSQL 竞价事务操作失败".to_owned())
    }
}

pub(crate) fn is_serialization_error(error: &sqlx::Error) -> bool {
    matches!(
        error,
        sqlx::Error::Database(database_error)
            if matches!(database_error.code().as_deref(), Some("40001" | "40P01"))
    )
}

pub(crate) fn is_unique_error(error: &sqlx::Error) -> bool {
    matches!(error, sqlx::Error::Database(database_error) if database_error.code().as_deref() == Some("23505"))
}

pub(crate) fn invalid_record() -> CompetitionTransactionError {
    CompetitionTransactionError::CommandLedger(CommandLedgerError::InvalidRecord)
}

pub(crate) fn invalid_state(message: &str) -> CompetitionTransactionError {
    CompetitionTransactionError::Failed(message.to_owned())
}

pub(crate) fn parse_auction_status(
    value: String,
) -> Result<AuctionStatus, CompetitionTransactionError> {
    match value.as_str() {
        "draft" => Ok(AuctionStatus::Draft),
        "preview" => Ok(AuctionStatus::Preview),
        "live" => Ok(AuctionStatus::Live),
        "final" => Ok(AuctionStatus::Final),
        "settling" => Ok(AuctionStatus::Settling),
        "settled" => Ok(AuctionStatus::Settled),
        "cancelled" => Ok(AuctionStatus::Cancelled),
        _ => Err(invalid_state("拍卖状态不在允许集合内")),
    }
}
