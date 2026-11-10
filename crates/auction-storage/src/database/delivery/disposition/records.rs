//! 投递处置事实的安全只读投影。

use auction_application::core::ApplicationError;
use auction_application::delivery::DeliveryDispositionRecord;
use chrono::{DateTime, Utc};
use sqlx::{Postgres, Row};

use crate::database::{TrackedPool, storage_error_from};

/// 只读取安全列；不含 payload、正文、凭据或原始证据。
pub(crate) async fn dispositions_for_delivery(
    pool: &TrackedPool<Postgres>,
    delivery_id: uuid::Uuid,
) -> Result<Vec<DeliveryDispositionRecord>, ApplicationError> {
    let rows = sqlx::query(
        "SELECT delivery_id, fencing_version, kind, reason_code, actor_source, actor_id,
                reason, resolves_alert, occurred_at
         FROM public.delivery_dispositions
         WHERE delivery_id = $1
         ORDER BY occurred_at DESC, id DESC",
    )
    .bind(delivery_id)
    .fetch_all(pool)
    .await
    .map_err(|error| storage_error_from(error, "读取投递处置事实失败"))?;
    rows.iter()
        .map(|row| {
            Ok(DeliveryDispositionRecord {
                delivery_id: row
                    .try_get("delivery_id")
                    .map_err(|error| storage_error_from(error, "读取投递处置投递标识失败"))?,
                fencing_version: row
                    .try_get("fencing_version")
                    .map_err(|error| storage_error_from(error, "读取投递处置 fence 失败"))?,
                kind: row
                    .try_get("kind")
                    .map_err(|error| storage_error_from(error, "读取投递处置种类失败"))?,
                reason_code: row
                    .try_get("reason_code")
                    .map_err(|error| storage_error_from(error, "读取投递处置原因码失败"))?,
                actor_source: row
                    .try_get("actor_source")
                    .map_err(|error| storage_error_from(error, "读取投递处置操作者来源失败"))?,
                actor_id: row
                    .try_get("actor_id")
                    .map_err(|error| storage_error_from(error, "读取投递处置操作者失败"))?,
                reason: row
                    .try_get("reason")
                    .map_err(|error| storage_error_from(error, "读取投递处置原因失败"))?,
                resolves_alert: row
                    .try_get("resolves_alert")
                    .map_err(|error| storage_error_from(error, "读取投递处置解除标记失败"))?,
                occurred_at: row
                    .try_get::<DateTime<Utc>, _>("occurred_at")
                    .map_err(|error| storage_error_from(error, "读取投递处置时间失败"))?,
            })
        })
        .collect()
}
