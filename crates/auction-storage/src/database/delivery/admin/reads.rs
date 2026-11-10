//! PostgreSQL delivery Admin 列表读取。

use auction_application::delivery::{
    DeliveryAdminError, DeliveryAdminListRequest, DeliveryAdminPage,
};
use sqlx::Postgres;

use super::map_storage_error;
use super::mapping::snapshot;
use crate::database::TrackedPool;

/// 列表与计数共用同一组精确筛选；参数顺序固定为
/// status、reference、recipient、template、category、world、created_from、created_to。
/// reference 同时匹配投递 ID、结算义务 ID 与玩家通知 ID，其余筛选都是等值匹配。
/// 用宏展开而不是动态拼接，让 SQLx 的静态字符串检查覆盖完整语句。
macro_rules! delivery_filters {
    () => {
        r#"
           WHERE ($1::text IS NULL OR deliveries.status = $1::text)
             AND (
                 $2::uuid IS NULL
                 OR deliveries.id = $2::uuid
                 OR (
                     deliveries.origin_kind IN ('settlement', 'settlement_recovery_action')
                     AND deliveries.origin_id = $2::text
                 )
                 OR EXISTS (
                     SELECT 1 FROM public.lot_watch_notifications AS notification
                     WHERE notification.id = $2::uuid
                       AND notification.delivery_id = deliveries.id
                 )
             )
             AND ($3::text IS NULL OR lower(deliveries.recipient) = lower($3::text))
             AND ($4::text IS NULL OR deliveries.payload_template = $4::text)
             AND (
                 $5::text IS NULL
                 OR public.delivery_send_category(deliveries.origin_kind, deliveries.payload) = $5::text
             )
             AND (
                 $6::text IS NULL
                 OR COALESCE(deliveries.payload_world, 'main') = $6::text
             )
             AND ($7::timestamptz IS NULL OR deliveries.created_at >= $7::timestamptz)
             AND ($8::timestamptz IS NULL OR deliveries.created_at <= $8::timestamptz)"#
    };
}

const COUNT_SQL: &str =
    concat!("SELECT count(*)::bigint FROM public.deliveries", delivery_filters!());

const PAGE_SQL: &str = concat!(
    r#"SELECT id, origin_kind, origin_id, recipient, payload, result_metadata, status,
              lease_token,
              lease_expires_at, fencing_version, attempt_count, claimed_at,
              sending_at, completed_at, external_reference, failure_code,
              created_at, updated_at
       FROM public.deliveries"#,
    delivery_filters!(),
    r#"
       ORDER BY created_at DESC, id DESC
       LIMIT $9 OFFSET $10"#
);

pub(crate) async fn list(
    pool: &TrackedPool<Postgres>,
    request: &DeliveryAdminListRequest,
) -> Result<DeliveryAdminPage, DeliveryAdminError> {
    // Worker 持续迁移投递状态；筛选计数与当前页不能分属两个数据库快照。
    let mut transaction = pool.begin_read_snapshot().await.map_err(map_storage_error)?;
    let mut count = sqlx::query_scalar::<_, i64>(COUNT_SQL);
    let mut rows_query = sqlx::query(PAGE_SQL);
    let reference = request.reference;
    let recipient = request.recipient.as_deref();
    let template = request.template.as_deref();
    let category = request.category.as_deref();
    let world = request.world.as_deref();
    let status = request.status.as_deref();
    let created_from = request.created_from;
    let created_to = request.created_to;
    count = count
        .bind(status)
        .bind(reference)
        .bind(recipient)
        .bind(template)
        .bind(category)
        .bind(world)
        .bind(created_from)
        .bind(created_to);
    rows_query = rows_query
        .bind(status)
        .bind(reference)
        .bind(recipient)
        .bind(template)
        .bind(category)
        .bind(world)
        .bind(created_from)
        .bind(created_to)
        .bind(request.limit)
        .bind(request.offset);
    let total = count.fetch_one(&mut *transaction).await.map_err(map_storage_error)?;
    let rows = rows_query.fetch_all(&mut *transaction).await.map_err(map_storage_error)?;
    transaction.commit().await.map_err(map_storage_error)?;
    let entries = rows.iter().map(snapshot).collect::<Result<Vec<_>, _>>()?;
    Ok(DeliveryAdminPage { entries, total, limit: request.limit, offset: request.offset })
}
