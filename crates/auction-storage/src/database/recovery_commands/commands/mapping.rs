//! Recovery command 的动作映射与 SQL 错误转换。
//!
//! 该文件只承载 PostgreSQL recovery command 的单一职责，事务装配仍由父模块统一负责。

#[cfg(feature = "postgres")]
use super::*;

#[cfg(feature = "postgres")]
pub(super) fn confirmation_action_kind(
    world_id: &str,
    action: RecoveryConfirmationAction,
) -> Option<&'static str> {
    match (world_id, action) {
        ("main", RecoveryConfirmationAction::Reclaimed) => Some("reclaim_equipment_take"),
        ("isekai", RecoveryConfirmationAction::Cancelled) => Some("cancel_carrier_take"),
        ("isekai", RecoveryConfirmationAction::EquipmentRecovered) => {
            Some("recover_equipment_take")
        }
        _ => None,
    }
}

#[cfg(feature = "postgres")]
pub(super) fn action_confirmation(
    world_id: &str,
    kind: &str,
) -> Option<RecoveryConfirmationAction> {
    match (world_id, kind) {
        ("main", "reclaim_equipment_take") => Some(RecoveryConfirmationAction::Reclaimed),
        ("isekai", "cancel_carrier_take") => Some(RecoveryConfirmationAction::Cancelled),
        ("isekai", "recover_equipment_take") => {
            Some(RecoveryConfirmationAction::EquipmentRecovered)
        }
        _ => None,
    }
}

#[cfg(feature = "postgres")]
pub(super) fn map_insert_error(error: sqlx::Error) -> RecoveryCommandError {
    if let sqlx::Error::Database(database) = &error {
        // 只有 command_id 主键冲突表示另一个相同命令已经提交；recovery/action
        // 等业务唯一约束冲突不能被误判为可重放的命令冲突。
        if database.code().as_deref() == Some("23505")
            && database.constraint() == Some("settlement_recovery_commands_pkey")
        {
            return RecoveryCommandError::CommandConflict;
        }
    }
    map_sql_error(error)
}

#[cfg(feature = "postgres")]
pub(super) fn map_sql_error(error: sqlx::Error) -> RecoveryCommandError {
    if matches!(&error, sqlx::Error::Database(database) if database.code().as_deref() == Some("40001"))
    {
        RecoveryCommandError::SerializationConflict
    } else {
        RecoveryCommandError::Failed("PostgreSQL recovery 确认事务失败".to_owned())
    }
}
