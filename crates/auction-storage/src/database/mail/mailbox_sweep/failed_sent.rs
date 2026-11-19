//! Admin 判失败投递的只读取证轮转。详情尝试先按投递与 MID 持久登记，不授权任何外部写入。

use std::collections::HashMap;

use auction_application::core::ApplicationError;
use auction_application::delivery::SentAdmissionSnapshot;
use auction_application::mail::{
    MoogleMailboxFailedSend, MoogleMailboxFailedSendSlot, MoogleMailboxSentEntry,
    MoogleMailboxSweepPosition,
};
use auction_time::Timestamp;
use serde_json::Value;
use sqlx::Postgres;
use uuid::Uuid;

use super::candidates::record_sweep_attempt;
use crate::database::delivery::sent_confirmation::{
    SENT_OBSERVATION_KINDS, admission_rows, decode_sent_observation,
};
use crate::database::operations::audit_health::AuditWriteResult as _;
use crate::database::storage_error_from;
use crate::database::{TrackedPool, TrackedTransaction as Transaction};

pub(super) const DETAIL_ATTEMPT_ACTION: &str = "delivery.sent_failed_detail_attempt";

/// 不以 fence 为键，后续对账或重排替换投递元数据也不能再次消费同一详情读取机会。
pub(super) fn detail_attempt_id(delivery_id: Uuid, mid: &str) -> Uuid {
    Uuid::new_v5(&delivery_id, format!("{DETAIL_ATTEMPT_ACTION}:main:{mid}").as_bytes())
}

/// 轮转先取有界的持久位置，再通过与确认一致的准入快照投影取证目标。
pub(super) async fn read_slots(
    pool: &TrackedPool<Postgres>,
    start: Option<MoogleMailboxSweepPosition>,
    oldest_visible: Timestamp,
    limit: i64,
) -> Result<Vec<MoogleMailboxFailedSendSlot>, ApplicationError> {
    let positions: Vec<(Uuid, i64)> = sqlx::query_as(
        "SELECT d.id, d.fencing_version
         FROM public.deliveries d
         JOIN public.settlement_obligation_fulfillments f ON f.delivery_id = d.id
         WHERE d.status = 'failed' AND d.origin_kind = 'settlement'
           AND d.payload_world = 'main' AND d.payload_operation = 'send'
           AND d.sending_at IS NOT NULL AND d.completed_at >= $1
           AND d.external_reference IS NULL
         ORDER BY CASE WHEN $3::timestamptz IS NULL
                        OR (d.created_at, d.id) > ($3, $4::uuid)
                       THEN 0 ELSE 1 END, d.created_at, d.id
         LIMIT $2",
    )
    .bind(oldest_visible)
    .bind(limit)
    .bind(start.map(|position| position.created_at))
    .bind(start.map(|position| position.delivery_id))
    .fetch_all(pool)
    .await
    .map_err(|error| storage_error_from(error, "MoogleMail 失败投递轮转查询失败"))?;
    if positions.is_empty() {
        return Ok(Vec::new());
    }
    let delivery_ids: Vec<_> = positions.iter().map(|(id, _)| *id).collect();
    let mut snapshots = HashMap::with_capacity(positions.len());
    for row in admission_rows(&delivery_ids, "main", None)
        .fetch_all(pool)
        .await
        .map_err(|error| storage_error_from(error, "MoogleMail 失败投递取证准入查询失败"))?
    {
        let snapshot: SentAdmissionSnapshot =
            serde_json::from_value(row).map_err(ApplicationError::Serialization)?;
        snapshots.insert(snapshot.delivery.id, snapshot);
    }
    Ok(positions
        .into_iter()
        .map(|(delivery_id, fencing_version)| {
            let target = snapshots.remove(&delivery_id).and_then(|snapshot| {
                if snapshot.delivery.fencing_version != fencing_version {
                    return None;
                }
                let (not_before, not_after) = snapshot.failed_sent_observation_window()?;
                Some(MoogleMailboxFailedSend {
                    expectation: snapshot.expectation?,
                    not_before,
                    not_after,
                })
            });
            MoogleMailboxFailedSendSlot { delivery_id, fencing_version, target }
        })
        .collect())
}

/// 复核当前 fence，推进原游标并登记详情读取机会。行锁在发出请求前释放。
pub(super) async fn begin_detail(
    pool: &TrackedPool<Postgres>,
    delivery_id: Uuid,
    fencing_version: i64,
    entry: Option<&MoogleMailboxSentEntry>,
) -> Result<bool, ApplicationError> {
    let Some(entry) = entry else {
        record_sweep_attempt(pool, "main", delivery_id, Some(fencing_version)).await?;
        return Ok(false);
    };
    let mut tx = pool.begin().await.map_err(detail_storage_error)?;
    let attempt_id = detail_attempt_id(delivery_id, &entry.external_mail_id);
    let current: Option<(Value, bool)> = sqlx::query_as(
        "SELECT d.result_metadata,
                EXISTS (SELECT 1 FROM public.audit_records a WHERE a.id = $3)
         FROM public.deliveries d
         WHERE d.id = $1 AND d.fencing_version = $2 AND d.status = 'failed'
         FOR UPDATE OF d",
    )
    .bind(delivery_id)
    .bind(fencing_version)
    .bind(attempt_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(detail_storage_error)?;
    let Some((metadata, attempted)) = current else {
        tx.rollback().await.map_err(detail_storage_error)?;
        return Ok(false);
    };
    let advanced =
        record_sweep_attempt(&mut *tx, "main", delivery_id, Some(fencing_version)).await?;
    let reserved = if advanced && !attempted {
        match has_observation(&metadata, &entry.external_mail_id) {
            Ok(false) => {
                reserve_detail(&mut tx, delivery_id, fencing_version, entry, attempt_id).await?
            }
            Ok(true) => false,
            Err(error) => {
                // 损坏证据只阻断当前候选，仍须提交游标才能继续轮转（不变量 5、6）。
                tracing::warn!(
                    %delivery_id,
                    fencing_version,
                    error_category = ?error.classify(),
                    "失败投递的 Sent 观察无法解码，跳过详情读取"
                );
                false
            }
        }
    } else {
        false
    };
    // 未知提交不返回授权。留证与一次性读取登记是分开的，后者不能证明外部内容（不变量 5、6）。
    tx.commit().await.map_err(detail_storage_error)?;
    Ok(reserved)
}

fn has_observation(metadata: &Value, mid: &str) -> Result<bool, serde_json::Error> {
    for kind in SENT_OBSERVATION_KINDS {
        if let Some(observation) = metadata.get(kind) {
            let decoded = decode_sent_observation(observation)?;
            if decoded.mid == mid {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

async fn reserve_detail(
    tx: &mut Transaction<'_, Postgres>,
    delivery_id: Uuid,
    fencing_version: i64,
    entry: &MoogleMailboxSentEntry,
    attempt_id: Uuid,
) -> Result<bool, ApplicationError> {
    let Some(row) = admission_rows(&[delivery_id], "main", Some(&entry.external_mail_id))
        .fetch_optional(&mut **tx)
        .await
        .map_err(detail_storage_error)?
    else {
        return Ok(false);
    };
    let snapshot: SentAdmissionSnapshot =
        serde_json::from_value(row).map_err(ApplicationError::Serialization)?;
    if !snapshot
        .failed_sent_observation_window()
        .is_some_and(|(start, end)| (start..=end).contains(&entry.sent_at))
    {
        return Ok(false);
    }
    let inserted = sqlx::query(
        "INSERT INTO public.audit_records
            (id, actor_type, actor_id, action, entity_type, entity_id, result, source, metadata)
         VALUES ($1, 'worker', 'mailbox_maintenance', $2, 'delivery', $3, 'success', 'worker',
                 jsonb_build_object('mid',$4::text,'fencing_version',$5::bigint))
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(attempt_id)
    .bind(DETAIL_ATTEMPT_ACTION)
    .bind(delivery_id.to_string())
    .bind(&entry.external_mail_id)
    .bind(fencing_version)
    .execute(&mut **tx)
    .await
    .record_audit_failure()
    .map_err(detail_storage_error)?;
    Ok(inserted.rows_affected() == 1)
}

fn detail_storage_error(error: sqlx::Error) -> ApplicationError {
    storage_error_from(error, "MoogleMail 失败投递详情登记未确认")
}
