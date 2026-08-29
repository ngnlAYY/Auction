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
    let status = request.status.as_deref();
    let total: i64 = sqlx::query_scalar(
        "SELECT count(*)::bigint FROM public.deliveries
         WHERE $1::text IS NULL OR status = $1::text",
    )
    .bind(status)
    .fetch_one(pool)
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
    .fetch_all(pool)
    .await
    .map_err(map_storage_error)?;
    let entries = rows.iter().map(snapshot).collect::<Result<Vec<_>, _>>()?;
    Ok(DeliveryAdminPage { entries, total, limit: request.limit, offset: request.offset })
}
