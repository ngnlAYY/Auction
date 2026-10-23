//! MoogleMail 维护的本地证据与受证明恢复；状态、资金、履约及追加审计同事务提交。

use auction_application::{
    ApplicationFuture, MoogleMailboxEvidenceStore, MoogleMailboxSweepCandidate,
    MoogleMailboxUnprovenSend, RepositoryError,
};
use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;

#[cfg(feature = "postgres")]
use super::delivery_audit::{DeliveryAudit, RELEASE, SUCCEED, append as append_delivery_audit};
#[cfg(feature = "postgres")]
use super::settlement_funding as funding;
use super::{DatabaseStore, TrackedPool};
#[cfg(feature = "postgres")]
use sqlx::Postgres;

impl MoogleMailboxEvidenceStore for DatabaseStore {
    fn unproven_sends<'a>(
        &'a self,
        world_id: &'a str,
        limit: i64,
    ) -> ApplicationFuture<'a, Vec<MoogleMailboxSweepCandidate>> {
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

    fn record_sweep_attempt<'a>(
        &'a self,
        world_id: &'a str,
        delivery_id: Uuid,
    ) -> ApplicationFuture<'a, bool> {
        Box::pin(async move {
            if !matches!(world_id, "main" | "isekai") || delivery_id.is_nil() {
                return Err(storage_error("MoogleMail 维护轮转参数无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    record_sweep_attempt(store.pool(), world_id, delivery_id).await
                }
            }
        })
    }

    fn begin_compose_discard(&self, delivery_id: Uuid) -> ApplicationFuture<'_, bool> {
        Box::pin(async move {
            if delivery_id.is_nil() {
                return Err(storage_error("MoogleMail 草稿丢弃围栏参数无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => begin_compose_discard(store.pool(), delivery_id).await,
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
    ) -> ApplicationFuture<'a, bool> {
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
    ) -> ApplicationFuture<'a, bool> {
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
    ) -> ApplicationFuture<'a, bool> {
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
    ) -> ApplicationFuture<'a, bool> {
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
/// result_code、登录 key 用户、入站 MID、发送围栏与未知结果记录时间。
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
) -> Result<Vec<MoogleMailboxSweepCandidate>, auction_application::ApplicationError> {
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
                completed_at,
                COALESCE(payload->>'operation', ''),
                CASE WHEN origin_kind='settlement' THEN (
                    SELECT CASE
                        WHEN count(e.id)=jsonb_array_length(
                            COALESCE(payload->'equipment_secret_refs','[]'::jsonb))
                        THEN COALESCE(jsonb_agg(e.external_equipment_id ORDER BY e.external_equipment_id)
                            FILTER (WHERE e.id IS NOT NULL),'[]'::jsonb)
                        ELSE 'null'::jsonb END
                    FROM jsonb_array_elements(
                        COALESCE(payload->'equipment_secret_refs','[]'::jsonb)) AS ref
                    LEFT JOIN public.equipment e ON e.id::text=ref->>'id'
                        AND ref->>'kind'='auction_equipment_key'
                        AND e.world_id=COALESCE(payload->>'world','main')
                ) ELSE COALESCE(payload->'attachment_ids','[]'::jsonb) END,
                COALESCE((result_metadata->>'sweep_retries')::bigint, 0)
         FROM public.deliveries
         LEFT JOIN public.mailbox_sweep_cursors sweep ON sweep.world_id = $1
         WHERE status = 'manual_reconcile'
           AND payload->>'operation' IN ('send', 'take')
           AND external_reference IS NULL
           AND sending_at IS NOT NULL
           AND completed_at IS NOT NULL
           AND origin_kind IN ('settlement', 'mail_intake')
           AND COALESCE(payload->>'world', 'main') = $1
         ORDER BY CASE WHEN sweep.last_created_at IS NULL
                            OR (created_at, id) > (sweep.last_created_at, sweep.last_delivery_id)
                       THEN 0 ELSE 1 END,
                  created_at, id
         LIMIT $2",
    )
    .bind(world_id)
    .bind(limit)
    .fetch_all(pool.raw())
    .await
    .map_err(|_| storage_error("MoogleMail 未证明投递查询失败"))?;
    // 每行只出现一次，即使批次跨过尾部回绕也不会在本轮重复核对。
    // 不可重建行仍返回轮转身份，避免固定坏前缀占满 200 条后永久隐藏后缀。
    Ok(rows
        .into_iter()
        .map(|row| MoogleMailboxSweepCandidate {
            delivery_id: row.0,
            send: recoverable_send(world_id, row),
        })
        .collect())
}

#[cfg(feature = "postgres")]
fn recoverable_send(world_id: &str, row: UnprovenSendRow) -> Option<MoogleMailboxUnprovenSend> {
    let (
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
        outcome_recorded_at,
        operation,
        attachment_value,
        sweep_retries,
    ) = row;
    if recipient.is_empty() || cod < 0 || credits < 0 || outcome_recorded_at < sending_at {
        return None;
    }
    let attachment_ids = serde_json::from_value::<Vec<String>>(attachment_value).ok()?;
    // 结算恢复靠义务 reference；通知恢复靠结果码重建正文；take 靠入站 MID 与附件。
    let usable = match (origin_kind.as_str(), operation.as_str()) {
        ("settlement", "send") => !reference.is_empty(),
        ("mail_intake", "send") => result_code.is_some() && !inbound_mail_id.is_empty(),
        ("mail_intake", "take") => {
            !inbound_mail_id.is_empty()
                && !attachment_ids.is_empty()
                && attachment_ids.iter().all(|id| {
                    !id.is_empty() && id.len() <= 32 && id.bytes().all(|byte| byte.is_ascii_digit())
                })
        }
        _ => false,
    };
    if !usable {
        return None;
    }
    Some(MoogleMailboxUnprovenSend {
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
        outcome_recorded_at,
        operation,
        attachment_ids,
        sweep_retries,
        reference,
        template,
    })
}

#[cfg(feature = "postgres")]
async fn record_sweep_attempt(
    pool: &TrackedPool<Postgres>,
    world_id: &str,
    delivery_id: Uuid,
) -> Result<bool, auction_application::ApplicationError> {
    // 只推进当前一条；取消或提交应答丢失最多消费本条机会，后缀位置不受影响。
    // 排序键来自原 delivery，不接受调用方伪造时间，也不锁住远端读取期间的业务行。
    let result = sqlx::query(
        "INSERT INTO public.mailbox_sweep_cursors (world_id, last_created_at, last_delivery_id)
         SELECT $1, created_at, id FROM public.deliveries
         WHERE id = $2 AND COALESCE(payload->>'world', 'main') = $1
           AND status = 'manual_reconcile' AND external_reference IS NULL
           AND payload->>'operation' IN ('send', 'take')
           AND sending_at IS NOT NULL AND completed_at IS NOT NULL
           AND origin_kind IN ('settlement', 'mail_intake')
         ON CONFLICT (world_id) DO UPDATE
         SET last_created_at = EXCLUDED.last_created_at,
             last_delivery_id = EXCLUDED.last_delivery_id",
    )
    .bind(world_id)
    .bind(delivery_id)
    .execute(pool.raw())
    .await
    .map_err(|_| storage_error("MoogleMail 维护轮转提交未确认"))?;
    Ok(result.rows_affected() == 1)
}

#[cfg(feature = "postgres")]
async fn begin_compose_discard(
    pool: &TrackedPool<Postgres>,
    delivery_id: Uuid,
) -> Result<bool, auction_application::ApplicationError> {
    // 一次性尝试围栏不是成功证明；即使后续尚未 POST 就取消，也保守保留，不自动重置。
    let result = sqlx::query(
        "UPDATE public.deliveries
         SET result_metadata = COALESCE(result_metadata, '{}'::jsonb)
             || jsonb_build_object('compose_discard_attempt',
                 jsonb_build_object('started_at', clock_timestamp()))
         WHERE id = $1 AND status = 'manual_reconcile' AND external_reference IS NULL
           AND payload->>'operation' = 'send'
           AND sending_at IS NOT NULL AND completed_at IS NOT NULL
           AND origin_kind IN ('settlement', 'mail_intake')
           AND COALESCE(payload->>'world', 'main') = 'main'
           AND COALESCE(payload->'variables'->>'reference', '') <> ''
           AND NOT (COALESCE(result_metadata, '{}'::jsonb) ? 'compose_discard_attempt')
           AND NOT (COALESCE(result_metadata, '{}'::jsonb) ? 'compose_discard')",
    )
    .bind(delivery_id)
    .execute(pool.raw())
    .await
    .map_err(|_| storage_error("MoogleMail 草稿丢弃围栏提交未确认"))?;
    Ok(result.rows_affected() == 1)
}

#[cfg(feature = "postgres")]
enum RecoveryUpdate<'a> {
    Send(&'a str, &'a Value),
    Notice(&'a str, &'a Value),
    Action(&'a str, &'a Value),
    Requeue(i64, &'a str),
}

#[cfg(feature = "postgres")]
async fn bind_recovered_action(
    pool: &TrackedPool<Postgres>,
    delivery_id: Uuid,
    external_mail_id: String,
    metadata: Value,
) -> Result<bool, auction_application::ApplicationError> {
    persist_transition(pool, delivery_id, RecoveryUpdate::Action(&external_mail_id, &metadata))
        .await
}

#[cfg(feature = "postgres")]
async fn requeue_unproven(
    pool: &TrackedPool<Postgres>,
    delivery_id: Uuid,
    retries: i64,
    reason: &str,
) -> Result<bool, auction_application::ApplicationError> {
    persist_transition(pool, delivery_id, RecoveryUpdate::Requeue(retries, reason)).await
}

#[cfg(feature = "postgres")]
async fn bind_recovered_notice(
    pool: &TrackedPool<Postgres>,
    delivery_id: Uuid,
    external_mail_id: String,
    metadata: Value,
) -> Result<bool, auction_application::ApplicationError> {
    persist_transition(pool, delivery_id, RecoveryUpdate::Notice(&external_mail_id, &metadata))
        .await
}

#[cfg(feature = "postgres")]
async fn record_observation(
    pool: &TrackedPool<Postgres>,
    delivery_id: Uuid,
    key: &str,
    observation: Value,
) -> Result<(), auction_application::ApplicationError> {
    // 观察只追加安全元数据；受证明状态迁移由独立事务同时更新资金、履约和审计。
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
) -> Result<bool, auction_application::ApplicationError> {
    persist_transition(pool, delivery_id, RecoveryUpdate::Send(&external_mail_id, &metadata)).await
}

/// 只重试明确的序列化冲突/死锁；提交结果未知必须报错，不能计为恢复成功或重发。
#[cfg(feature = "postgres")]
async fn persist_transition(
    pool: &TrackedPool<Postgres>,
    delivery_id: Uuid,
    update: RecoveryUpdate<'_>,
) -> Result<bool, auction_application::ApplicationError> {
    for attempt in 0..3 {
        match persist_transition_once(pool, delivery_id, &update).await {
            Ok(changed) => return Ok(changed),
            Err(error) if error.retryable() && attempt < 2 => continue,
            Err(_) => return Err(storage_error("MoogleMail 恢复事务未确定提交，请核对原投递")),
        }
    }
    Err(storage_error("MoogleMail 恢复事务重试已耗尽"))
}

#[cfg(feature = "postgres")]
async fn persist_transition_once(
    pool: &TrackedPool<Postgres>,
    delivery_id: Uuid,
    update: &RecoveryUpdate<'_>,
) -> Result<bool, RecoveryError> {
    let mut tx = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE").execute(&mut *tx).await?;
    // 先取得资金 scope/account，再锁 delivery；未知分配不能因恢复路径漏记而继续占用。
    // Serializable 的重复引用谓词同时阻止不同维护事务把同一 MID 绑定给两条投递。
    let account = funding::lock_funded_delivery_account(&mut tx, delivery_id).await?;
    sqlx::query("SELECT id FROM public.deliveries WHERE id=$1 FOR UPDATE")
        .bind(delivery_id)
        .fetch_optional(&mut *tx)
        .await?;
    let (origin_id, completed_at): (String, DateTime<Utc>) = match update {
        RecoveryUpdate::Requeue(retries, reason) => {
            let updated = sqlx::query_as(
                "UPDATE public.deliveries
                 SET status='pending', completed_at=NULL, failure_code=NULL, lease_token=NULL,
                     lease_expires_at=NULL, sending_at=NULL, updated_at=clock_timestamp(),
                     result_metadata=COALESCE(result_metadata, '{}'::jsonb)
                         || jsonb_build_object('sweep_retries',$2::bigint,
                             'sweep_requeued_at',clock_timestamp(),'sweep_requeue_reason',$3::text)
                 WHERE id=$1 AND status='manual_reconcile' AND origin_kind='mail_intake'
                   AND payload->>'operation'='take' AND external_reference IS NULL
                   AND COALESCE(payload->>'world','main')='main'
                   AND COALESCE((result_metadata->>'sweep_retries')::bigint,0)=$2-1
                 RETURNING origin_id,updated_at",
            )
            .bind(delivery_id)
            .bind(retries)
            .bind(reason)
            .fetch_optional(&mut *tx)
            .await?;
            let Some(row) = updated else {
                tx.rollback().await?;
                return Ok(false);
            };
            row
        }
        RecoveryUpdate::Send(mid, metadata)
        | RecoveryUpdate::Notice(mid, metadata)
        | RecoveryUpdate::Action(mid, metadata) => {
            let settlement = matches!(update, RecoveryUpdate::Send(..));
            let operation =
                if matches!(update, RecoveryUpdate::Action(..)) { "take" } else { "send" };
            let origin = if settlement { "settlement" } else { "mail_intake" };
            let reference = format!("mooglemail:main:{operation}:{mid}");
            let updated = sqlx::query_as(
                "UPDATE public.deliveries
                 SET external_reference=$2,status='succeeded',failure_code=NULL,
                     completed_at=clock_timestamp(),updated_at=clock_timestamp(),
                     lease_token=NULL,lease_expires_at=NULL,
                     result_metadata=COALESCE(result_metadata,'{}'::jsonb)||$3::jsonb
                 WHERE id=$1 AND status='manual_reconcile' AND origin_kind=$4
                   AND payload->>'operation'=$5 AND external_reference IS NULL
                   AND sending_at IS NOT NULL AND COALESCE(payload->>'world','main')='main'
                   AND ($5<>'take' OR payload->>'external_mail_id'=$6)
                   AND NOT EXISTS (
                       SELECT 1 FROM public.deliveries other WHERE other.external_reference=$2)
                 RETURNING origin_id,completed_at",
            )
            .bind(delivery_id)
            .bind(&reference)
            .bind(*metadata)
            .bind(origin)
            .bind(operation)
            .bind(*mid)
            .fetch_optional(&mut *tx)
            .await?;
            let Some(row) = updated else {
                tx.rollback().await?;
                return Ok(false);
            };
            row
        }
    };
    if matches!(update, RecoveryUpdate::Send(..)) {
        if let Some(account) = account {
            funding::finish_funded_delivery(
                &mut tx,
                &account,
                delivery_id,
                auction_application::FundingDeliveryOutcome::Sent,
            )
            .await?;
        }
        let obligation_id = Uuid::parse_str(&origin_id).map_err(|_| RecoveryError::Rejected)?;
        let enqueued = sqlx::query(
            "UPDATE public.settlement_obligation_fulfillments
             SET execution_state='enqueued',state_version=state_version+1,delivery_id=$2,
                 claim_token=NULL,claimed_at=NULL,claim_expires_at=NULL,
                 next_attempt_at=clock_timestamp(),last_error_code=NULL,completed_at=NULL,
                 updated_at=clock_timestamp()
             WHERE obligation_id=$1 AND execution_state='manual_reconcile'
               AND delivery_operation='send' AND delivery_id=$2",
        )
        .bind(obligation_id)
        .bind(delivery_id)
        .execute(&mut *tx)
        .await?;
        if enqueued.rows_affected() != 1 {
            return Err(RecoveryError::Rejected);
        }
    }
    let requeued = matches!(update, RecoveryUpdate::Requeue(..));
    append_delivery_audit(
        &mut tx,
        DeliveryAudit {
            occurred_at: completed_at,
            actor_type: "worker",
            actor_id: "mailbox-maintenance",
            action: if requeued { RELEASE } else { SUCCEED },
            delivery_id,
            request_id: None,
            source: "worker",
            metadata: serde_json::json!({
                "status": if requeued { "pending" } else { "succeeded" },
                "recovered": !requeued,
                "mailbox_maintenance": true,
            }),
        },
    )
    .await?;
    tx.commit().await?;
    Ok(true)
}

/// 保留 PostgreSQL SQLSTATE 与资金冲突分类至重试边界，不匹配错误文本。
#[cfg(feature = "postgres")]
enum RecoveryError {
    Sqlx(sqlx::Error),
    Funding(auction_application::FundingError),
    Rejected,
}

#[cfg(feature = "postgres")]
impl From<sqlx::Error> for RecoveryError {
    fn from(error: sqlx::Error) -> Self {
        Self::Sqlx(error)
    }
}

#[cfg(feature = "postgres")]
impl From<auction_application::FundingError> for RecoveryError {
    fn from(error: auction_application::FundingError) -> Self {
        Self::Funding(error)
    }
}

#[cfg(feature = "postgres")]
impl RecoveryError {
    fn retryable(&self) -> bool {
        match self {
            Self::Sqlx(error) => error
                .as_database_error()
                .and_then(|error| error.code())
                .is_some_and(|code| matches!(code.as_ref(), "40001" | "40P01")),
            Self::Funding(auction_application::FundingError::SerializationConflict) => true,
            _ => false,
        }
    }
}

fn storage_error(message: &str) -> auction_application::ApplicationError {
    RepositoryError::Failed(message.to_owned()).into()
}
