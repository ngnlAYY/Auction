//! 测试入口共用的时间、错误与旧原语夹具。

use auction_time::Timestamp;
use sqlx::Postgres;
use uuid::Uuid;

use crate::database::consignment::commands::errors::WriteError;
use crate::database::operations::audit_health::AuditWriteResult as _;
use crate::database::pool::TrackedTransaction;

pub(super) fn error_text(error: WriteError) -> String {
    format!("{:?}", error.into_application())
}

/// 夹具的期限均以 2026-01-02 为界；合成写入使用界内固定时刻。
pub(super) fn fixture_now() -> Result<Timestamp, WriteError> {
    Timestamp::parse_from_rfc3339("2026-01-01T03:00:00Z")
        .ok()
        .ok_or_else(|| WriteError::invariant("consignment_deposit_fixture_time"))
}

pub(super) async fn tracked_marker(
    transaction: &mut TrackedTransaction<'_, Postgres>,
    marker: Uuid,
) -> Result<(), WriteError> {
    sqlx::query(
        "INSERT INTO public.audit_records
            (id, actor_type, actor_id, action, entity_type, entity_id, result, source, metadata)
         VALUES ($1, 'system', 'deposit-test', 'test.consignment_deposit_tracked',
                 'consignment_command', $1::text, 'success', 'system', '{}'::jsonb)",
    )
    .bind(marker)
    .execute(&mut **transaction)
    .await
    .record_audit_failure()?;
    Ok(())
}
