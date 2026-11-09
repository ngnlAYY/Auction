//! PostgreSQL runtime 命令账本的只读查询。

#[cfg(feature = "postgres")]
use auction_application::admin::AdminRuntimeStoredCommand;
#[cfg(feature = "postgres")]
use auction_application::core::ApplicationError;
#[cfg(feature = "postgres")]
use auction_domain::CommandId;

#[cfg(feature = "postgres")]
use super::{errors::command_failed, mapping::map_command_row};

#[cfg(feature = "postgres")]
pub(super) const SELECT_COMMAND_BY_ID: &str =
    "SELECT command_id, digest_version, request_digest, state,
       expected_fingerprint, old_fingerprint, new_fingerprint, candidate_document,
       restart_required, audit_id, actor_id, request_id, occurred_at, unknown_category,
       created_at, updated_at, applied_at
FROM public.runtime_config_admin_commands
WHERE command_id = $1";

#[cfg(feature = "postgres")]
pub(super) const SELECT_UNRESOLVED: &str =
    "SELECT command_id, digest_version, request_digest, state,
       expected_fingerprint, old_fingerprint, new_fingerprint, candidate_document,
       restart_required, audit_id, actor_id, request_id, occurred_at, unknown_category,
       created_at, updated_at, applied_at
FROM public.runtime_config_admin_commands
WHERE state IN ('pending', 'unknown')
ORDER BY updated_at, command_id";

#[cfg(feature = "postgres")]
pub(super) const SELECT_COMMAND_FOR_UPDATE: &str =
    "SELECT command_id, digest_version, request_digest, state,
       expected_fingerprint, old_fingerprint, new_fingerprint, candidate_document,
       restart_required, audit_id, actor_id, request_id, occurred_at, unknown_category,
       created_at, updated_at, applied_at
FROM public.runtime_config_admin_commands
WHERE command_id = $1
FOR UPDATE";

#[cfg(feature = "postgres")]
pub(super) async fn find_postgres(
    store: &crate::DatabaseStore,
    command_id: CommandId,
) -> Result<Option<AdminRuntimeStoredCommand>, ApplicationError> {
    let row = sqlx::query(SELECT_COMMAND_BY_ID)
        .bind(command_id.as_uuid())
        .fetch_optional(store.pool())
        .await
        .map_err(|_| command_failed("读取 runtime 配置命令失败"))?;
    row.map(map_command_row).transpose()
}

#[cfg(feature = "postgres")]
pub(super) async fn list_unresolved_postgres(
    store: &crate::DatabaseStore,
) -> Result<Vec<AdminRuntimeStoredCommand>, ApplicationError> {
    let rows = sqlx::query(SELECT_UNRESOLVED)
        .fetch_all(store.pool())
        .await
        .map_err(|_| command_failed("读取未完成的 runtime 配置命令失败"))?;
    rows.into_iter().map(map_command_row).collect()
}
