//! 邮箱巡检的证据写入与受证明状态迁移入口。

use auction_application::core::ApplicationError;
use auction_application::delivery::{
    DeliverySendCredentialBinding, DeliverySendExpectation, DeliveryTakeExpectation,
};
use serde_json::Value;
use sqlx::Postgres;
use uuid::Uuid;

use super::error::RecoveryError;
use super::failed_sent;
use super::recovery;
use super::transition::{RecoveryUpdate, persist_transition};
use crate::database::TrackedPool;
use crate::database::storage_error_from;

pub(super) async fn begin_compose_discard(
    pool: &TrackedPool<Postgres>,
    delivery_id: Uuid,
    fencing_version: i64,
    expectation: Option<&DeliverySendExpectation>,
) -> Result<bool, ApplicationError> {
    // 一次性尝试不是远端成功证明；仅明确中止的冲突可以重试，不重放未知提交。
    crate::database::retry_conflicts(
        pool,
        auction_observability::DatabaseTransactionOwner::MailboxSweep,
        crate::database::SERIALIZABLE_ATTEMPTS,
        RecoveryError::retryable,
        || recovery::begin_discard_once(pool, delivery_id, fencing_version, expectation),
    )
    .await
    .map_err(|error| error.into_application_error("MoogleMail 草稿丢弃围栏提交未确认"))
}

pub(super) async fn bind_recovered_action(
    pool: &TrackedPool<Postgres>,
    delivery_id: Uuid,
    fencing_version: i64,
    external_mail_id: String,
    metadata: Value,
    expectation: &DeliveryTakeExpectation,
    observation: &DeliverySendCredentialBinding,
) -> Result<bool, ApplicationError> {
    persist_transition(
        pool,
        delivery_id,
        RecoveryUpdate::Action(
            &external_mail_id,
            &metadata,
            fencing_version,
            expectation,
            observation,
        ),
    )
    .await
}

pub(super) async fn requeue_unproven(
    pool: &TrackedPool<Postgres>,
    delivery_id: Uuid,
    fencing_version: i64,
    retries: i64,
    reason: &str,
    expectation: &DeliveryTakeExpectation,
    observation: &DeliverySendCredentialBinding,
) -> Result<bool, ApplicationError> {
    persist_transition(
        pool,
        delivery_id,
        RecoveryUpdate::Requeue(retries, reason, fencing_version, expectation, observation),
    )
    .await
}

pub(super) async fn record_observation(
    pool: &TrackedPool<Postgres>,
    delivery_id: Uuid,
    fencing_version: i64,
    key: &str,
    observation: Value,
    sighting_mid: Option<&str>,
) -> Result<bool, ApplicationError> {
    // 覆盖与保留实见附件在同一 UPDATE 内完成；旧响应不能附着到新 fence，
    // 同一 MID 清空后也不能丢掉此前可用于核对装备身份的编号（不变量 6）。
    let result = sqlx::query(
        "UPDATE public.deliveries AS delivery
         SET result_metadata = COALESCE(delivery.result_metadata, '{}'::jsonb)
             || jsonb_build_object($2::text, $3::jsonb || COALESCE((
                 SELECT jsonb_build_object('pre_collection_equipment_ids', previous.ids)
                 FROM (VALUES
                     (delivery.result_metadata -> $2::text, 0),
                     (delivery.result_metadata -> CASE $2::text
                         WHEN 'mailbox_sweep' THEN 'sent_reference_sighting'
                         WHEN 'sent_reference_sighting' THEN 'mailbox_sweep' END, 1)
                 ) AS history(observation, priority)
                 CROSS JOIN LATERAL (SELECT COALESCE(
                     NULLIF(NULLIF(history.observation->'remaining_equipment_ids', '[]'::jsonb),
                            'null'::jsonb),
                     history.observation->'pre_collection_equipment_ids') AS ids) AS previous
                 WHERE $2::text IN ('mailbox_sweep', 'sent_reference_sighting')
                   AND $3::jsonb->'collected' = 'true'::jsonb
                   AND COALESCE(history.observation->>'mid', history.observation->>'external_mail_id')
                       = $3::jsonb->>'mid'
                   AND jsonb_typeof(previous.ids) = 'array' AND previous.ids <> '[]'::jsonb
                 ORDER BY history.priority LIMIT 1
             ), '{}'::jsonb))
         WHERE delivery.id = $1 AND delivery.fencing_version = $4
           AND (delivery.status = 'manual_reconcile' OR (
               delivery.status = 'failed' AND $5::text IS NOT NULL
               AND EXISTS (SELECT 1 FROM public.audit_records AS attempt
                   WHERE attempt.id = $6 AND attempt.action = $7
                     AND attempt.entity_id = delivery.id::text
                     AND attempt.metadata->>'mid' = $5
                     AND attempt.metadata->'fencing_version' = to_jsonb($4::bigint))))
           AND ($5::text IS NULL OR (
               delivery.origin_kind = 'settlement' AND delivery.payload_operation = 'send'
               AND NOT EXISTS (SELECT 1 FROM public.delivery_sent_bindings binding
                   WHERE binding.world_id = COALESCE(delivery.payload_world, 'main')
                     AND binding.external_mail_id = $5 AND binding.delivery_id <> delivery.id)
               AND NOT EXISTS (SELECT 1 FROM public.deliveries other
                   WHERE other.id <> delivery.id
                     AND other.external_reference =
                         'mooglemail:' || COALESCE(delivery.payload_world, 'main') || ':send:' || $5)))",
    )
    .bind(delivery_id)
    .bind(key)
    .bind(observation)
    .bind(fencing_version)
    .bind(sighting_mid)
    .bind(sighting_mid.map(|mid| failed_sent::detail_attempt_id(delivery_id, mid)))
    .bind(failed_sent::DETAIL_ATTEMPT_ACTION)
    .execute(pool)
    .await
    .map_err(|error| {
        let message = if sighting_mid.is_some() {
            "MoogleMail Sent 目击写入失败"
        } else {
            "MoogleMail 维护证据写入失败"
        };
        storage_error_from(error, message)
    })?;
    // 已换 fence 的迟到观察影响零行，不能计为本轮已取得的证据（不变量 6）。
    Ok(result.rows_affected() == 1)
}

pub(super) async fn bind_recovered_send(
    pool: &TrackedPool<Postgres>,
    delivery_id: Uuid,
    fencing_version: i64,
    external_mail_id: String,
    metadata: Value,
    expectation: Option<&DeliverySendExpectation>,
) -> Result<bool, ApplicationError> {
    persist_transition(
        pool,
        delivery_id,
        RecoveryUpdate::Send(&external_mail_id, &metadata, fencing_version, expectation),
    )
    .await
}
