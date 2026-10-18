//! MoogleMail 三页维护的本地读取与追加证据；只追加 `result_metadata`，不改变投递状态。

use auction_application::{
    ApplicationFuture, MoogleMailboxEvidenceStore, MoogleMailboxUnprovenSend, RepositoryError,
};
use chrono::{DateTime, Utc};
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

    fn bind_recovered_action<'a>(
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
                return Err(storage_error("MoogleMail 动作恢复证明参数无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    bind_recovered_action(store.pool(), delivery_id, external_mail_id, metadata)
                        .await
                }
            }
        })
    }

    fn requeue_unproven<'a>(
        &'a self,
        delivery_id: Uuid,
        retries: i64,
        reason: &'a str,
    ) -> ApplicationFuture<'a, ()> {
        Box::pin(async move {
            if delivery_id.is_nil() || !(0..=8).contains(&retries) || reason.is_empty() {
                return Err(storage_error("MoogleMail 维护重排队参数无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    requeue_unproven(store.pool(), delivery_id, retries, reason).await
                }
            }
        })
    }

    fn bind_recovered_notice<'a>(
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
                return Err(storage_error("MoogleMail 通知恢复证明参数无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    bind_recovered_notice(store.pool(), delivery_id, external_mail_id, metadata)
                        .await
                }
            }
        })
    }
}

/// 未证明投递的扁平行：id、来源、收件人、COD、credits、reference、模板、
/// result_code、登录 key 用户、入站 MID、发送围栏时间。
#[cfg(feature = "postgres")]
type UnprovenSendRow = (
    Uuid,
    String,
    String,
    i64,
    i64,
    String,
    String,
    Option<String>,
    Option<Uuid>,
    String,
    DateTime<Utc>,
    String,
    serde_json::Value,
    i64,
);

#[cfg(feature = "postgres")]
async fn unproven_sends(
    pool: &TrackedPool<Postgres>,
    world_id: &str,
    limit: i64,
) -> Result<Vec<MoogleMailboxUnprovenSend>, auction_application::ApplicationError> {
    // 只暴露可供比对的最小安全字段：来源、收件人、COD、reference 与通知渲染引用；
    // 正文密文、登录 key 明文都不出库。
    let rows: Vec<UnprovenSendRow> = sqlx::query_as(
        "SELECT id,
                origin_kind,
                recipient,
                COALESCE((payload->>'cod')::bigint, 0),
                COALESCE((payload->>'credits')::bigint, 0),
                COALESCE(payload->'variables'->>'reference', ''),
                COALESCE(payload->>'template', ''),
                payload->>'result_code',
                NULLIF(payload->>'user_key_user_id', '')::uuid,
                COALESCE(payload->>'external_mail_id', ''),
                sending_at,
                COALESCE(payload->>'operation', ''),
                COALESCE(payload->'attachment_ids', '[]'::jsonb),
                COALESCE((result_metadata->>'sweep_retries')::bigint, 0)
         FROM public.deliveries
         WHERE status = 'manual_reconcile'
           AND payload->>'operation' IN ('send', 'take')
           AND external_reference IS NULL
           AND sending_at IS NOT NULL
           AND origin_kind IN ('settlement', 'mail_intake')
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
    for (
        delivery_id,
        origin_kind,
        recipient,
        cod,
        credits,
        reference,
        template,
        result_code,
        user_key_user_id,
        inbound_mail_id,
        sending_at,
        operation,
        attachment_value,
        sweep_retries,
    ) in rows
    {
        if recipient.is_empty() || cod < 0 || credits < 0 {
            continue;
        }
        let Ok(attachment_ids) = serde_json::from_value::<Vec<String>>(attachment_value) else {
            continue;
        };
        // 结算恢复靠义务 reference；通知恢复靠结果码重建正文；take 靠入站 MID 与附件。
        let usable = match (origin_kind.as_str(), operation.as_str()) {
            ("settlement", "send") => !reference.is_empty(),
            ("mail_intake", "send") => result_code.is_some() && !inbound_mail_id.is_empty(),
            ("mail_intake", "take") => {
                !inbound_mail_id.is_empty()
                    && !attachment_ids.is_empty()
                    && attachment_ids.iter().all(|id| {
                        !id.is_empty()
                            && id.len() <= 32
                            && id.bytes().all(|byte| byte.is_ascii_digit())
                    })
            }
            _ => false,
        };
        if !usable {
            continue;
        }
        sends.push(MoogleMailboxUnprovenSend {
            delivery_id,
            origin_kind,
            world_id: world_id.to_owned(),
            recipient,
            cod,
            credits,
            inbound_mail_id,
            result_code,
            user_key_user_id,
            sending_at,
            operation,
            attachment_ids,
            sweep_retries,
            reference,
            template,
        });
    }
    Ok(sends)
}

#[cfg(feature = "postgres")]
async fn bind_recovered_action(
    pool: &TrackedPool<Postgres>,
    delivery_id: Uuid,
    external_mail_id: String,
    metadata: Value,
) -> Result<(), auction_application::ApplicationError> {
    let reference = format!("mooglemail:main:take:{external_mail_id}");
    // 入站邮件已不在收件箱：附件已随邮件消失，take 动作视为完成；只收口投递本身。
    let _updated: Option<Uuid> = sqlx::query_scalar(
        "UPDATE public.deliveries
         SET external_reference = $2,
             status = 'succeeded',
             failure_code = NULL,
             completed_at = clock_timestamp(),
             lease_token = NULL,
             lease_expires_at = NULL,
             result_metadata = COALESCE(result_metadata, '{}'::jsonb) || $3::jsonb
         WHERE id = $1
           AND status = 'manual_reconcile'
           AND origin_kind = 'mail_intake'
           AND payload->>'operation' = 'take'
           AND external_reference IS NULL
           AND NOT EXISTS (
               SELECT 1 FROM public.deliveries AS other
               WHERE other.external_reference = $2
           )
         RETURNING id",
    )
    .bind(delivery_id)
    .bind(&reference)
    .bind(&metadata)
    .fetch_optional(pool.raw())
    .await
    .map_err(|_| storage_error("MoogleMail 动作恢复证明写回失败"))?;
    Ok(())
}

#[cfg(feature = "postgres")]
async fn requeue_unproven(
    pool: &TrackedPool<Postgres>,
    delivery_id: Uuid,
    retries: i64,
    reason: &str,
) -> Result<(), auction_application::ApplicationError> {
    // 重排队必须回到活动态：清空完成时间与发送围栏，保留外部引用为空与累计次数。
    let _updated: Option<Uuid> = sqlx::query_scalar(
        "UPDATE public.deliveries
         SET status = 'pending',
             completed_at = NULL,
             failure_code = NULL,
             lease_token = NULL,
             lease_expires_at = NULL,
             sending_at = NULL,
             result_metadata = COALESCE(result_metadata, '{}'::jsonb)
                 || jsonb_build_object(
                     'sweep_retries', $2::bigint,
                     'sweep_requeued_at', clock_timestamp(),
                     'sweep_requeue_reason', $3::text
                 ),
             updated_at = clock_timestamp()
         WHERE id = $1
           AND status = 'manual_reconcile'
           AND origin_kind = 'mail_intake'
           AND COALESCE((result_metadata->>'sweep_retries')::bigint, 0) = $2 - 1
         RETURNING id",
    )
    .bind(delivery_id)
    .bind(retries)
    .bind(reason)
    .fetch_optional(pool.raw())
    .await
    .map_err(|_| storage_error("MoogleMail 维护重排队失败"))?;
    Ok(())
}

#[cfg(feature = "postgres")]
async fn bind_recovered_notice(
    pool: &TrackedPool<Postgres>,
    delivery_id: Uuid,
    external_mail_id: String,
    metadata: Value,
) -> Result<(), auction_application::ApplicationError> {
    let reference = format!("mooglemail:main:send:{external_mail_id}");
    // 通知类没有履约事实可重新排队：只收口投递本身，且必须仍处于待人工核对、
    // 未写回外部引用、发送围栏已建立的原始状态。
    let _updated: Option<Uuid> = sqlx::query_scalar(
        "UPDATE public.deliveries
         SET external_reference = $2,
             status = 'succeeded',
             failure_code = NULL,
             completed_at = clock_timestamp(),
             result_metadata = COALESCE(result_metadata, '{}'::jsonb) || $3::jsonb
         WHERE id = $1
           AND status = 'manual_reconcile'
           AND origin_kind = 'mail_intake'
           AND payload->>'operation' = 'send'
           AND external_reference IS NULL
           AND sending_at IS NOT NULL
           AND NOT EXISTS (
               SELECT 1 FROM public.deliveries AS other
               WHERE other.external_reference = $2
           )
         RETURNING id",
    )
    .bind(delivery_id)
    .bind(&reference)
    .bind(&metadata)
    .fetch_optional(pool.raw())
    .await
    .map_err(|_| storage_error("MoogleMail 通知恢复证明写回失败"))?;
    // 同一 MID 已绑定给别的投递时保持原状：维护是幂等追加，不应让整轮失败。
    Ok(())
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
             -- 重新排队属于回到活动态：manual_reconcile 写入的完成时间必须清空，
             -- 否则违反“活动态 completed_at 必须为 NULL”的检查约束。
             completed_at = NULL,
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
