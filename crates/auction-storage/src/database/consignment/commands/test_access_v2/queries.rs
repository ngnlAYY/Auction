//! PG 测试读取账本与挂单结果的只读辅助。

use sqlx::Row;
use uuid::Uuid;

use crate::database::DatabaseStore;

/// 测试辅助：读取审计标记是否存在。
pub async fn test_deposit_marker_exists(store: &DatabaseStore, marker: Uuid) -> bool {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM public.audit_records WHERE id = $1)")
        .bind(marker)
        .fetch_one(store.postgres_pool())
        .await
        .unwrap_or(false)
}

/// 测试辅助：检查挂单状态与版本。
pub async fn test_deposit_listing_state(
    store: &DatabaseStore,
    listing_id: Uuid,
) -> Result<(String, u64), String> {
    let row =
        sqlx::query("SELECT state, state_version FROM public.consignment_listings WHERE id = $1")
            .bind(listing_id)
            .fetch_one(store.postgres_pool())
            .await
            .map_err(|error| format!("{error:?}"))?;
    Ok((
        row.try_get("state").map_err(|error| format!("{error:?}"))?,
        row.try_get::<i64, _>("state_version")
            .map_err(|error| format!("{error:?}"))?
            .cast_unsigned(),
    ))
}

/// 测试辅助：统计一条命令的事件数。
pub async fn test_deposit_event_count(
    store: &DatabaseStore,
    command_id: Uuid,
) -> Result<i64, String> {
    sqlx::query_scalar("SELECT count(*) FROM public.consignment_events WHERE command_id = $1")
        .bind(command_id)
        .fetch_one(store.postgres_pool())
        .await
        .map_err(|error| format!("{error:?}"))
}

/// 测试辅助：检查命令是否存在。
pub async fn test_deposit_command_exists(store: &DatabaseStore, command_id: Uuid) -> bool {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM public.consignment_commands WHERE command_id = $1)",
    )
    .bind(command_id)
    .fetch_one(store.postgres_pool())
    .await
    .unwrap_or(false)
}

/// 测试辅助：统计某条命令追加的审计条数。
pub async fn test_deposit_audit_count(
    store: &DatabaseStore,
    command_id: Uuid,
) -> Result<i64, String> {
    sqlx::query_scalar(
        // 能定位挂单的命令把审计实体写成 `consignment_listing`；命令身份以 metadata 的
        // command_id 为唯一链接（与 Admin 读模型的 ADMIN_JOIN 同口径）。
        "SELECT count(*) FROM public.audit_records
         WHERE metadata->>'command_id' = $1::text",
    )
    .bind(command_id)
    .fetch_one(store.postgres_pool())
    .await
    .map_err(|error| format!("{error:?}"))
}

/// 测试辅助：统计某条命令被拒绝的审计条数。
pub async fn test_deposit_rejected_audit_count(
    store: &DatabaseStore,
    command_id: Uuid,
) -> Result<i64, String> {
    sqlx::query_scalar(
        // 同 `test_deposit_audit_count`：实体可能是挂单，命令身份只看 metadata.command_id。
        "SELECT count(*) FROM public.audit_records
         WHERE metadata->>'command_id' = $1::text
           AND result = 'rejected'",
    )
    .bind(command_id)
    .fetch_one(store.postgres_pool())
    .await
    .map_err(|error| format!("{error:?}"))
}
