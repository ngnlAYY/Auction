//! PostgreSQL delivery Admin 列表读取。

use auction_application::{DeliveryAdminError, DeliveryAdminListRequest, DeliveryAdminPage};
use sqlx::Postgres;

use super::map_storage_error;
use super::mapping::snapshot;
use crate::database::TrackedPool;

pub(crate) async fn list(
    pool: &TrackedPool<Postgres>,
    request: &DeliveryAdminListRequest,
) -> Result<DeliveryAdminPage, DeliveryAdminError> {
    // Worker 持续迁移投递状态；筛选计数与当前页不能分属两个数据库快照。
    let mut transaction = pool.begin().await.map_err(map_storage_error)?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
        .execute(&mut *transaction)
        .await
        .map_err(map_storage_error)?;
    let status = request.status.as_deref();
    let total: i64 = sqlx::query_scalar(
        "SELECT count(*)::bigint FROM public.deliveries
         WHERE $1::text IS NULL OR status = $1::text",
    )
    .bind(status)
    .fetch_one(&mut *transaction)
    .await
    .map_err(map_storage_error)?;
    let rows = sqlx::query(
        r#"SELECT id, origin_kind, origin_id, recipient, payload, result_metadata, status,
                  lease_token,
                  lease_expires_at, fencing_version, attempt_count, claimed_at,
                  sending_at, completed_at, external_reference, failure_code,
                  created_at, updated_at
           FROM public.deliveries
           WHERE $1::text IS NULL OR status = $1::text
           ORDER BY created_at DESC, id DESC
           LIMIT $2 OFFSET $3"#,
    )
    .bind(status)
    .bind(request.limit)
    .bind(request.offset)
    .fetch_all(&mut *transaction)
    .await
    .map_err(map_storage_error)?;
    transaction.commit().await.map_err(map_storage_error)?;
    let entries = rows.iter().map(snapshot).collect::<Result<Vec<_>, _>>()?;
    Ok(DeliveryAdminPage { entries, total, limit: request.limit, offset: request.offset })
}
