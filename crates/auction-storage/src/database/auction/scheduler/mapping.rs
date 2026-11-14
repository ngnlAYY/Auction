//! Worker 调度共享阶段、命令、UUID、时间与错误映射。

use auction_application::scheduling::ScheduledRoundPhase;
use auction_domain::{CommandId, deterministic_uuid, legacy_natural_key_uuid};
use chrono::NaiveDate;
use uuid::Uuid;

pub(crate) fn bounded_text(value: String, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

/// 轮次在 world 内的身份片段：早鸟场次用时段键，常规周沿用 `YYYY-MM-DD`。
pub(crate) fn round_identity(local_week: NaiveDate, slot_key: Option<&str>) -> String {
    slot_key.map_or_else(|| local_week.format("%Y-%m-%d").to_string(), str::to_owned)
}

pub(crate) fn command_id(
    create_command_id: Uuid,
    world_id: &str,
    round_identity: &str,
    phase: ScheduledRoundPhase,
) -> CommandId {
    let legacy_kind = match phase {
        ScheduledRoundPhase::Preview => Some("preview-command"),
        ScheduledRoundPhase::Open => Some("open-command"),
        ScheduledRoundPhase::Final => None,
    };
    if let Some(kind) = legacy_kind {
        // 常规周保持旧版 world/week 命令身份；同周多场的早鸟场次改用时段键，避免命令 ID 冲突。
        return CommandId::from_uuid(legacy_natural_key_uuid(
            "auction-schedule",
            &[kind.as_bytes(), world_id.as_bytes(), round_identity.as_bytes()],
        ));
    }

    // 旧版周计划只拥有 preview/open 命令；Final 是新版持久化 lifecycle 的内部阶段。
    CommandId::from_uuid(deterministic_uuid(
        CommandId::from_uuid(create_command_id),
        "scheduled-final",
        1,
    ))
}

pub(crate) fn phase_from_completion(
    preview_completed: bool,
    open_completed: bool,
) -> ScheduledRoundPhase {
    if !preview_completed {
        ScheduledRoundPhase::Preview
    } else if !open_completed {
        ScheduledRoundPhase::Open
    } else {
        ScheduledRoundPhase::Final
    }
}

pub(crate) use crate::database::storage_error;

pub(crate) use crate::database::repository_error;

pub(crate) fn map_storage_error(error: sqlx::Error) -> auction_application::core::RepositoryError {
    crate::database::storage_failure(&error, "Worker 调度记录不存在", "Worker 调度数据库操作失败")
}
