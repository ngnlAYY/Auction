//! MoogleMail 三页维护的本地读取与追加证据；只追加 `result_metadata`，不改变投递状态。

use auction_application::{
    ApplicationFuture, MoogleMailboxEvidenceStore, MoogleMailboxUnprovenSend, RepositoryError,
};
use serde_json::Value;
use uuid::Uuid;

use super::{DatabaseStore, TrackedPool};
#[cfg(feature = "postgres")]
use sqlx::Postgres;

impl MoogleMailboxEvidenceStore for DatabaseStore {
    fn unproven_sends<'a>(
        &'a self,
        world_id: &'a str,
        limit: i64,
    ) -> ApplicationFuture<'a, Vec<MoogleMailboxUnprovenSend>> {
        Box::pin(async move {
            if !matches!(world_id, "main" | "isekai") || !(1..=500).contains(&limit) {
                return Err(storage_error("MoogleMail 维护查询参数无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => unproven_sends(store.pool(), world_id, limit).await,
            }
        })
    }

    fn record_sent_observation<'a>(
        &'a self,
        delivery_id: Uuid,
        observation: Value,
    ) -> ApplicationFuture<'a, ()> {
        Box::pin(async move {
            if delivery_id.is_nil() || !observation.is_object() {
                return Err(storage_error("MoogleMail Sent 观察参数无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    record_observation(store.pool(), delivery_id, "mailbox_sweep", observation)
                        .await
                }
            }
        })
    }

    fn record_compose_discard<'a>(
        &'a self,
        delivery_id: Uuid,
        observation: Value,
    ) -> ApplicationFuture<'a, ()> {
        Box::pin(async move {
            if delivery_id.is_nil() || !observation.is_object() {
                return Err(storage_error("MoogleMail 草稿丢弃证据参数无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    record_observation(store.pool(), delivery_id, "compose_discard", observation)
                        .await
                }
            }
        })
    }

    fn bind_recovered_send<'a>(
        &'a self,
        delivery_id: Uuid,
        external_mail_id: String,
        metadata: Value,
    ) -> ApplicationFuture<'a, ()> {
        Box::pin(async move {
            if delivery_id.is_nil()
                || external_mail_id.is_empty()
                || external_mail_id.len() > 64
                || !external_mail_id.bytes().all(|byte| byte.is_ascii_digit())
                || !metadata.is_object()
            {
                return Err(storage_error("MoogleMail 恢复证明参数无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    bind_recovered_send(store.pool(), delivery_id, external_mail_id, metadata).await
                }
            }
        })
    }
}

#[cfg(feature = "postgres")]
async fn unproven_sends(
    pool: &TrackedPool<Postgres>,
    world_id: &str,
    limit: i64,
) -> Result<Vec<MoogleMailboxUnprovenSend>, auction_application::ApplicationError> {
    // 只暴露可供比对的最小安全字段：收件人、COD 与业务 reference；正文与密文不出库。
    let rows: Vec<(Uuid, String, i64, String, String)> = sqlx::query_as(
        "SELECT id,
                COALESCE(payload->>'recipient', ''),
                COALESCE((payload->>'cod')::bigint, 0),
                COALESCE(payload->>'reference', ''),
                COALESCE(payload->>'template', '')
         FROM public.deliveries
         WHERE status = 'manual_reconcile'
           AND payload->>'operation' = 'send'
           AND external_reference IS NULL
           AND sending_at IS NOT NULL
           AND COALESCE(payload->>'world', 'main') = $1
         ORDER BY created_at
         LIMIT $2",
    )
    .bind(world_id)
    .bind(limit)
    .fetch_all(pool.raw())
    .await
    .map_err(|_| storage_error("MoogleMail 未证明投递查询失败"))?;
    let mut sends = Vec::with_capacity(rows.len());
    for (delivery_id, recipient, cod, reference, template) in rows {
        if recipient.is_empty() || reference.is_empty() || cod < 0 {
            continue;
        }
        sends.push(MoogleMailboxUnprovenSend {
            delivery_id,
            world_id: world_id.to_owned(),
            recipient,
            cod,
            reference,
            template,
        });
    }
    Ok(sends)
}

#[cfg(feature = "postgres")]
async fn record_observation(
    pool: &TrackedPool<Postgres>,
    delivery_id: Uuid,
    key: &str,
    observation: Value,
) -> Result<(), auction_application::ApplicationError> {
    // 追加键不改 status、fencing、失败码或租约；人工 reconcile 仍是唯一状态迁移入口。
    sqlx::query(
        "UPDATE public.deliveries
         SET result_metadata = COALESCE(result_metadata, '{}'::jsonb)
             || jsonb_build_object($2::text, $3::jsonb)
         WHERE id = $1 AND status = 'manual_reconcile'",
    )
    .bind(delivery_id)
    .bind(key)
    .bind(observation)
    .execute(pool.raw())
    .await
    .map_err(|_| storage_error("MoogleMail 维护证据写入失败"))?;
    Ok(())
}

#[cfg(feature = "postgres")]
async fn bind_recovered_send(
    pool: &TrackedPool<Postgres>,
    delivery_id: Uuid,
    external_mail_id: String,
    metadata: Value,
) -> Result<(), auction_application::ApplicationError> {
    let reference = format!("mooglemail:main:send:{external_mail_id}");
    let mut tx =
        pool.raw().begin().await.map_err(|_| storage_error("MoogleMail 恢复证明事务失败"))?;
    let obligation: Option<String> = sqlx::query_scalar(
        "UPDATE public.deliveries
         SET external_reference = $2,
             status = 'succeeded',
             failure_code = NULL,
             completed_at = clock_timestamp(),
             result_metadata = COALESCE(result_metadata, '{}'::jsonb) || $3::jsonb
         WHERE id = $1
           AND status = 'manual_reconcile'
           AND origin_kind = 'settlement'
           AND payload->>'operation' = 'send'
           AND external_reference IS NULL
           AND sending_at IS NOT NULL
         RETURNING origin_id",
    )
    .bind(delivery_id)
    .bind(&reference)
    .bind(&metadata)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| storage_error("MoogleMail 恢复证明写回失败"))?;
    let Some(obligation) = obligation else {
        // 已被其他路径收口或不是可恢复投递时保持原样。
        tx.rollback().await.map_err(|_| storage_error("MoogleMail 恢复证明回滚失败"))?;
        return Ok(());
    };
    let Ok(obligation_id) = Uuid::parse_str(&obligation) else {
        tx.rollback().await.map_err(|_| storage_error("MoogleMail 恢复证明回滚失败"))?;
        return Err(storage_error("MoogleMail 恢复证明义务身份无效"));
    };
    let enqueued: Option<Uuid> = sqlx::query_scalar(
        "UPDATE public.settlement_obligation_fulfillments
         SET execution_state = 'enqueued',
             state_version = state_version + 1,
             delivery_id = $2,
             claim_token = NULL,
             claimed_at = NULL,
             claim_expires_at = NULL,
             next_attempt_at = clock_timestamp(),
             last_error_code = NULL,
             updated_at = clock_timestamp()
         WHERE obligation_id = $1
           AND execution_state = 'manual_reconcile'
           AND delivery_operation = 'send'
         RETURNING obligation_id",
    )
    .bind(obligation_id)
    .bind(delivery_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| storage_error("MoogleMail 恢复证明重新排队失败"))?;
    if enqueued.is_none() {
        tx.rollback().await.map_err(|_| storage_error("MoogleMail 恢复证明回滚失败"))?;
        return Err(storage_error("MoogleMail 恢复证明缺少可重新排队的履约事实"));
    }
    tx.commit().await.map_err(|_| storage_error("MoogleMail 恢复证明提交失败"))?;
    Ok(())
}

fn storage_error(message: &str) -> auction_application::ApplicationError {
    RepositoryError::Failed(message.to_owned()).into()
}
