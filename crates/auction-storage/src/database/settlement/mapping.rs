//! 结算策略、枚举和值转换与错误映射。
//!
//! 该文件只承载 PostgreSQL 结算事务的单一职责，事务边界仍由父模块统一装配。

use super::*;

pub(super) fn parse_settlement_policy(
    snapshot: &Value,
) -> Result<(SettlementPolicy, RecoveryPolicy), SettlementTransactionError> {
    let settlement = snapshot
        .get("settlement")
        .and_then(Value::as_object)
        .ok_or_else(|| invalid("拍卖策略快照缺少 settlement"))?;
    let version = json_u32(settlement.get("version"), "结算策略版本")?;
    let mode = match settlement.get("mode").and_then(Value::as_str) {
        Some("legacy_zero_v0") => SettlementMode::LegacyZeroV0,
        Some("hold_curve_v1") => SettlementMode::HoldCurveV1,
        _ => return Err(invalid("结算策略模式未知")),
    };
    let compensation_pool_enabled = settlement
        .get("compensation_pool_enabled")
        .and_then(Value::as_bool)
        .ok_or_else(|| invalid("补偿池策略字段无效"))?;
    let isekai_carrier_credit =
        json_u32(settlement.get("isekai_carrier_credit"), "isekai carrier credit")?;
    let anchors = settlement
        .get("anchors")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("结算锚点字段无效"))?
        .iter()
        .map(|anchor| {
            let object = anchor.as_object().ok_or_else(|| invalid("结算锚点不是对象"))?;
            Ok(auction_domain::AdjustmentAnchor {
                hold_ppm: json_i64(object.get("hold_ppm"), "结算锚点 hold_ppm")?,
                adjustment_ppm: json_i64(object.get("adjustment_ppm"), "结算锚点 adjustment_ppm")?,
            })
        })
        .collect::<Result<Vec<_>, SettlementTransactionError>>()?;
    let policy = SettlementPolicy {
        version,
        mode,
        compensation_pool_enabled,
        isekai_carrier_credit,
        anchors,
    };
    policy.validate().map_err(|_| invalid("结算策略快照校验失败"))?;

    let recovery = snapshot
        .get("recovery")
        .and_then(Value::as_object)
        .ok_or_else(|| invalid("拍卖策略快照缺少 recovery"))?;
    let recovery_policy = RecoveryPolicy {
        version: json_u32(recovery.get("version"), "恢复策略版本")?,
        mode: match recovery.get("mode").and_then(Value::as_str) {
            Some("remind_v1") => auction_domain::RecoveryMode::RemindV1,
            _ => return Err(invalid("恢复策略模式未知")),
        },
        payment_timeout: chrono::Duration::seconds(json_i64(
            recovery.get("payment_timeout_seconds"),
            "恢复付款超时",
        )?),
        reminder_limit: json_u32(recovery.get("reminder_limit"), "恢复提醒上限")?,
    };
    recovery_policy.validate().map_err(|_| invalid("恢复策略快照校验失败"))?;

    Ok((policy, recovery_policy))
}

pub(super) fn settlement_world(
    value: String,
) -> Result<SettlementWorld, SettlementTransactionError> {
    match value.as_str() {
        "main" => Ok(SettlementWorld::Main),
        "isekai" => Ok(SettlementWorld::Isekai),
        _ => Err(invalid("世界标识不支持结算")),
    }
}

pub(super) fn settlement_world_name(world: SettlementWorld) -> &'static str {
    match world {
        SettlementWorld::Main => "main",
        SettlementWorld::Isekai => "isekai",
    }
}

pub(super) fn settlement_mode(mode: SettlementMode) -> &'static str {
    match mode {
        SettlementMode::LegacyZeroV0 => "legacy_zero_v0",
        SettlementMode::HoldCurveV1 => "hold_curve_v1",
    }
}

pub(super) fn obligation_kind(kind: SettlementObligationKind) -> &'static str {
    match kind {
        SettlementObligationKind::WinnerEquipmentCod => "winner_equipment_cod",
        SettlementObligationKind::WinnerPaymentCarrierCod => "winner_payment_carrier_cod",
        SettlementObligationKind::WinnerEquipmentHandoff => "winner_equipment_handoff",
        SettlementObligationKind::SellerPayout => "seller_payout",
        SettlementObligationKind::BuyerRefund => "buyer_refund",
        SettlementObligationKind::UnsoldReturn => "unsold_return",
        SettlementObligationKind::WithdrawnReturn => "withdrawn_return",
    }
}

pub(super) fn obligation_state(state: SettlementObligationState) -> &'static str {
    match state {
        SettlementObligationState::Ready => "ready",
        SettlementObligationState::Blocked => "blocked",
        SettlementObligationState::Contingent => "contingent",
    }
}

pub(super) fn json_u32(
    value: Option<&Value>,
    label: &str,
) -> Result<u32, SettlementTransactionError> {
    let value = value.and_then(Value::as_i64).ok_or_else(|| invalid(label))?;
    u32::try_from(value).map_err(|_| invalid(label))
}

pub(super) fn json_i64(
    value: Option<&Value>,
    label: &str,
) -> Result<i64, SettlementTransactionError> {
    value.and_then(Value::as_i64).ok_or_else(|| invalid(label))
}

pub(super) fn positive_u64(value: i64, label: &str) -> Result<u64, SettlementTransactionError> {
    if value <= 0 {
        return Err(invalid(label));
    }
    u64::try_from(value).map_err(|_| invalid(label))
}

pub(super) fn i64_version(value: u64, label: &str) -> Result<i64, SettlementTransactionError> {
    i64::try_from(value).map_err(|_| invalid(label))
}

pub(super) fn i32_version(value: u32, label: &str) -> Result<i32, SettlementTransactionError> {
    i32::try_from(value).map_err(|_| invalid(label))
}

pub(super) fn invalid(message: &str) -> SettlementTransactionError {
    SettlementTransactionError::Failed(message.to_owned())
}

pub(super) fn map_competition_error(
    error: auction_application::CompetitionTransactionError,
) -> SettlementTransactionError {
    match error {
        auction_application::CompetitionTransactionError::NotFound => {
            SettlementTransactionError::NotFound
        }
        auction_application::CompetitionTransactionError::Conflict => {
            SettlementTransactionError::Conflict
        }
        auction_application::CompetitionTransactionError::StaleState
        | auction_application::CompetitionTransactionError::InvalidState => {
            SettlementTransactionError::Failed("竞价事务中的管理状态不允许当前结算操作".to_owned())
        }
        auction_application::CompetitionTransactionError::Failed(message) => {
            SettlementTransactionError::Failed(message)
        }
        auction_application::CompetitionTransactionError::SerializationConflict => {
            SettlementTransactionError::SerializationConflict
        }
        auction_application::CompetitionTransactionError::CommandLedger(error) => {
            SettlementTransactionError::CommandLedger(error)
        }
    }
}

pub(super) fn map_sql_error(error: sqlx::Error) -> SettlementTransactionError {
    let error = crate::database::competition::map_sql_error(error);
    map_competition_error(error)
}
