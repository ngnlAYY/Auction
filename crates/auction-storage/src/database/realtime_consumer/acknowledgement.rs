//! Realtime consumer 游标确认事务；只有 event_id 与 stream/sequence 完全匹配且序号连续才推进游标，
//! 失败记录的正常确认同事务收口，replay 事件仍由显式恢复流程处理。

#[cfg(feature = "postgres")]
use crate::database::storage_error_from;

use auction_application::core::ApplicationError;
#[cfg(feature = "postgres")]
use sqlx::Connection;
use uuid::Uuid;

use super::common::storage_error;
#[cfg(feature = "postgres")]
pub(super) async fn acknowledge_postgres(
    connection: &mut sqlx::PgConnection,
    consumer_key: &str,
    stream_key: &str,
    event_id: Uuid,
    stream_sequence: i64,
) -> Result<(), ApplicationError> {
    let transaction = connection
        .begin()
        .await
        .map_err(|error| storage_error_from(error, "打开 PostgreSQL 实时游标事务失败"))?;
    let mut transaction = crate::database::TrackedTransaction::borrowed(transaction);
    let result = acknowledge_postgres_inner(
        &mut transaction,
        consumer_key,
        stream_key,
        event_id,
        stream_sequence,
    )
    .await;
    match result {
        Ok(()) => transaction
            .commit()
            .await
            .map_err(|error| storage_error_from(error, "提交 PostgreSQL 实时游标事务失败")),
        Err(error) => {
            let _ = transaction.rollback().await;
            Err(error)
        }
    }
}

#[cfg(feature = "postgres")]
async fn acknowledge_postgres_inner(
    transaction: &mut crate::database::TrackedTransaction<'_, sqlx::Postgres>,
    consumer_key: &str,
    stream_key: &str,
    event_id: Uuid,
    stream_sequence: i64,
) -> Result<(), ApplicationError> {
    // 与 replay 保持 failure → consumer offset 锁序，不能先锁游标再反向等待同一 failure。
    // 身份或连续性检查失败时由唯一调用方回滚整事务，提前收口不会独立提交。
    sqlx::query(
        "UPDATE public.realtime_failures
         SET resolved_at = COALESCE(resolved_at, CURRENT_TIMESTAMP)
         WHERE consumer_key = $1 AND event_id = $2 AND replay_event_id IS NULL",
    )
    .bind(consumer_key)
    .bind(event_id)
    .execute(&mut **transaction)
    .await
    .map_err(|error| storage_error_from(error, "解决 PostgreSQL 实时消费失败记录失败"))?;
    let current = sqlx::query_scalar::<_, i64>(
        "SELECT last_sequence
         FROM public.realtime_consumer_offsets
         WHERE consumer_key = $1 AND stream_key = $2
         FOR UPDATE",
    )
    .bind(consumer_key)
    .bind(stream_key)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|error| storage_error_from(error, "锁定 PostgreSQL 实时消费游标失败"))?
    .unwrap_or(0);
    let stored_event_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT event_id
         FROM public.realtime_events
         WHERE stream_key = $1 AND stream_sequence = $2",
    )
    .bind(stream_key)
    .bind(stream_sequence)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|error| storage_error_from(error, "校验 PostgreSQL 实时事件序号失败"))?;
    if stored_event_id != Some(event_id) {
        return Err(storage_error("PostgreSQL 实时事件不存在或 stream 不匹配"));
    }
    if stream_sequence > current {
        let expected =
            current.checked_add(1).ok_or_else(|| storage_error("PostgreSQL 实时消费游标溢出"))?;
        if stream_sequence != expected {
            return Err(storage_error("PostgreSQL 实时消费游标不能跳过事件"));
        }
        sqlx::query(
            "INSERT INTO public.realtime_consumer_offsets
                (consumer_key, stream_key, last_sequence, updated_at)
             VALUES ($1, $2, $3, CURRENT_TIMESTAMP)
             ON CONFLICT (consumer_key, stream_key) DO UPDATE
             SET last_sequence = EXCLUDED.last_sequence, updated_at = CURRENT_TIMESTAMP",
        )
        .bind(consumer_key)
        .bind(stream_key)
        .bind(stream_sequence)
        .execute(&mut **transaction)
        .await
        .map_err(|error| storage_error_from(error, "写入 PostgreSQL 实时消费游标失败"))?;
    }
    Ok(())
}
