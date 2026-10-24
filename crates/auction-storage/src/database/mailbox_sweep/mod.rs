//! MoogleMail 维护的本地证据与受证明恢复；状态、资金、履约及追加审计同事务提交。

use auction_application::{
    ApplicationFuture, DeliverySendCredentialBinding, DeliverySendExpectation,
    DeliveryTakeExpectation, MoogleMailboxEvidenceStore, MoogleMailboxSweepCandidate,
    MoogleMailboxUnprovenSend, RepositoryError,
};
use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;

#[cfg(feature = "postgres")]
use super::delivery_audit::{DeliveryAudit, RELEASE, SUCCEED, append as append_delivery_audit};
#[cfg(feature = "postgres")]
use super::delivery_scheduler::{attempts, take_attempts};
#[cfg(feature = "postgres")]
use super::settlement_funding as funding;
use super::{DatabaseStore, TrackedPool};
#[cfg(feature = "postgres")]
use sqlx::Postgres;

#[cfg(feature = "postgres")]
mod recovery;

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

    fn begin_compose_discard<'a>(
        &'a self,
        delivery_id: Uuid,
        fencing_version: i64,
        expectation: Option<&'a DeliverySendExpectation>,
    ) -> ApplicationFuture<'a, bool> {
        Box::pin(async move {
            if delivery_id.is_nil()
                || fencing_version <= 0
                || expectation.is_some_and(|expectation| !expectation.validate())
            {
                return Err(storage_error("MoogleMail 草稿丢弃围栏参数无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    begin_compose_discard(store.pool(), delivery_id, fencing_version, expectation)
                        .await
                }
            }
        })
    }

    fn record_sent_observation<'a>(
        &'a self,
        delivery_id: Uuid,
        fencing_version: i64,
        observation: Value,
    ) -> ApplicationFuture<'a, ()> {
        Box::pin(async move {
            if delivery_id.is_nil() || fencing_version <= 0 || !observation.is_object() {
                return Err(storage_error("MoogleMail Sent 观察参数无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    record_observation(
                        store.pool(),
                        delivery_id,
                        fencing_version,
                        "mailbox_sweep",
                        observation,
                    )
                    .await
                }
            }
        })
    }

    fn record_compose_discard<'a>(
        &'a self,
        delivery_id: Uuid,
        fencing_version: i64,
        observation: Value,
    ) -> ApplicationFuture<'a, ()> {
        Box::pin(async move {
            if delivery_id.is_nil() || fencing_version <= 0 || !observation.is_object() {
                return Err(storage_error("MoogleMail 草稿丢弃证据参数无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    record_observation(
                        store.pool(),
                        delivery_id,
                        fencing_version,
                        "compose_discard",
                        observation,
                    )
                    .await
                }
            }
        })
    }

    fn bind_recovered_send<'a>(
        &'a self,
        delivery_id: Uuid,
        fencing_version: i64,
        external_mail_id: String,
        metadata: Value,
        expectation: Option<&'a DeliverySendExpectation>,
    ) -> ApplicationFuture<'a, bool> {
        Box::pin(async move {
            if delivery_id.is_nil()
                || fencing_version <= 0
                || external_mail_id.is_empty()
                || external_mail_id.len() > 64
                || !external_mail_id.bytes().all(|byte| byte.is_ascii_digit())
                || !metadata.is_object()
                || expectation.is_some_and(|expectation| !expectation.validate())
            {
                return Err(storage_error("MoogleMail 恢复证明参数无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    bind_recovered_send(
                        store.pool(),
                        delivery_id,
                        fencing_version,
                        external_mail_id,
                        metadata,
                        expectation,
                    )
                    .await
                }
            }
        })
    }

    fn bind_recovered_action<'a>(
        &'a self,
        delivery_id: Uuid,
        fencing_version: i64,
        external_mail_id: String,
        metadata: Value,
        expectation: &'a DeliveryTakeExpectation,
        observation: &'a DeliverySendCredentialBinding,
    ) -> ApplicationFuture<'a, bool> {
        Box::pin(async move {
            if delivery_id.is_nil()
                || fencing_version <= 0
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
                    bind_recovered_action(
                        store.pool(),
                        delivery_id,
                        fencing_version,
                        external_mail_id,
                        metadata,
                        expectation,
                        observation,
                    )
                    .await
                }
            }
        })
    }

    fn requeue_unproven<'a>(
        &'a self,
        delivery_id: Uuid,
        fencing_version: i64,
        retries: i64,
        reason: &'a str,
        expectation: &'a DeliveryTakeExpectation,
        observation: &'a DeliverySendCredentialBinding,
    ) -> ApplicationFuture<'a, bool> {
        Box::pin(async move {
            if delivery_id.is_nil()
                || fencing_version <= 0
                || !(0..=8).contains(&retries)
                || reason.is_empty()
            {
                return Err(storage_error("MoogleMail 维护重排队参数无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    requeue_unproven(
                        store.pool(),
                        delivery_id,
                        fencing_version,
                        retries,
                        reason,
                        expectation,
                        observation,
                    )
                    .await
                }
            }
        })
    }
}

/// 每次尝试的冻结身份优先；旧行只保留既有安全恢复所需的最小投影。
#[cfg(feature = "postgres")]
#[derive(sqlx::FromRow)]
struct UnprovenSendRow {
    delivery_id: Uuid,
    origin_kind: String,
    recipient: String,
    cod: i64,
    credits: i64,
    reference: String,
    template: String,
    result_code: Option<String>,
    user_key_user_id: Option<Uuid>,
    inbound_mail_id: String,
    sending_at: DateTime<Utc>,
    outcome_recorded_at: DateTime<Utc>,
    operation: String,
    attachment_value: Value,
    sweep_retries: i64,
    fencing_version: i64,
    sending_fencing_version: i64,
    has_send_attempt: bool,
    expectation: Option<Value>,
    has_take_attempt: bool,
    take_expectation: Option<Value>,
}

#[cfg(feature = "postgres")]
async fn unproven_sends(
    pool: &TrackedPool<Postgres>,
    world_id: &str,
    limit: i64,
) -> Result<Vec<MoogleMailboxSweepCandidate>, auction_application::ApplicationError> {
    // 所有 Send 来源共享冻结身份；不按 origin 白名单漏掉 watch、alert 或恢复发送。
    // 仅旧结算解析既有装备引用；正文和登录 key 不进入维护投影。
    let rows: Vec<UnprovenSendRow> = sqlx::query_as(
        "SELECT d.id AS delivery_id,
                d.origin_kind,
                d.recipient,
                COALESCE((d.payload->>'cod')::bigint, 0) AS cod,
                COALESCE((d.payload->>'credits')::bigint, 0) AS credits,
                COALESCE(d.payload->'variables'->>'reference', '') AS reference,
                COALESCE(d.payload->>'template', '') AS template,
                d.payload->>'result_code' AS result_code,
                NULLIF(d.payload->>'user_key_user_id', '')::uuid AS user_key_user_id,
                COALESCE(d.payload->>'external_mail_id', '') AS inbound_mail_id,
                d.sending_at,
                d.completed_at AS outcome_recorded_at,
                COALESCE(d.payload->>'operation', '') AS operation,
                CASE WHEN attempt.expectation IS NOT NULL
                    THEN attempt.expectation->'equipment_ids'
                WHEN d.origin_kind='settlement' THEN (
                    SELECT CASE
                        WHEN count(e.id)=jsonb_array_length(
                            COALESCE(d.payload->'equipment_secret_refs','[]'::jsonb))
                        THEN COALESCE(jsonb_agg(e.external_equipment_id ORDER BY e.external_equipment_id)
                            FILTER (WHERE e.id IS NOT NULL),'[]'::jsonb)
                        ELSE 'null'::jsonb END
                    FROM jsonb_array_elements(
                        COALESCE(d.payload->'equipment_secret_refs','[]'::jsonb)) AS ref
                    LEFT JOIN public.equipment e ON e.id::text=ref->>'id'
                        AND ref->>'kind'='auction_equipment_key'
                        AND e.world_id=COALESCE(d.payload->>'world','main')
                ) ELSE COALESCE(d.payload->'attachment_ids','[]'::jsonb) END AS attachment_value,
                COALESCE((d.result_metadata->>'sweep_retries')::bigint, 0) AS sweep_retries,
                d.fencing_version,
                COALESCE(attempt.sending_fencing_version,take_attempt.taking_fencing_version,
                         d.fencing_version) AS sending_fencing_version,
                EXISTS (SELECT 1 FROM public.delivery_send_attempts history
                        WHERE history.delivery_id=d.id) AS has_send_attempt,
                attempt.expectation,
                EXISTS (SELECT 1 FROM public.delivery_take_attempts history
                        WHERE history.delivery_id=d.id) AS has_take_attempt,
                take_attempt.expectation AS take_expectation
         FROM public.deliveries d
         LEFT JOIN LATERAL public.delivery_current_send_attempt(d.id) attempt ON true
         LEFT JOIN LATERAL public.delivery_current_take_attempt(d.id) take_attempt ON true
         LEFT JOIN public.mailbox_sweep_cursors sweep ON sweep.world_id = $1
         WHERE d.status = 'manual_reconcile'
           AND d.payload->>'operation' IN ('send', 'take')
           AND d.external_reference IS NULL
           AND d.sending_at IS NOT NULL
           AND d.completed_at IS NOT NULL
           AND COALESCE(d.payload->>'world', 'main') = $1
         ORDER BY CASE WHEN sweep.last_created_at IS NULL
                            OR (d.created_at, d.id) > (sweep.last_created_at, sweep.last_delivery_id)
                       THEN 0 ELSE 1 END,
                  d.created_at, d.id
         LIMIT $2",
    )
    .bind(world_id)
    .bind(limit)
    .fetch_all(pool.raw())
    .await
    .map_err(|_| storage_error("MoogleMail 未证明投递查询失败"))?;
    // 不可重建行仍返回轮转身份，避免固定坏前缀永久隐藏后缀。
    Ok(rows
        .into_iter()
        .map(|row| MoogleMailboxSweepCandidate {
            delivery_id: row.delivery_id,
            send: recoverable_send(world_id, row),
        })
        .collect())
}

#[cfg(feature = "postgres")]
fn recoverable_send(world_id: &str, row: UnprovenSendRow) -> Option<MoogleMailboxUnprovenSend> {
    let UnprovenSendRow {
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
        fencing_version,
        sending_fencing_version,
        has_send_attempt,
        expectation,
        has_take_attempt,
        take_expectation,
    } = row;
    if recipient.is_empty() || cod < 0 || credits < 0 || outcome_recorded_at < sending_at {
        return None;
    }
    if (has_send_attempt && expectation.is_none())
        || (has_take_attempt && take_expectation.is_none())
    {
        // 当前发送签名损坏或已换尝试，不能把新格式记录降级成旧模板恢复。
        return None;
    }
    let attachment_ids = serde_json::from_value::<Vec<String>>(attachment_value).ok()?;
    let send_expectation = match expectation {
        Some(value) => {
            let frozen: DeliverySendExpectation = serde_json::from_value(value).ok()?;
            if !frozen.validate()
                || frozen.world_id != world_id
                || frozen.recipient != recipient
                || frozen.cod != cod
                || frozen.credits != credits
                || frozen.equipment_ids != attachment_ids
                || operation != "send"
            {
                return None;
            }
            Some(frozen)
        }
        None => None,
    };
    let take_expectation = match take_expectation {
        Some(value) => {
            let frozen: DeliveryTakeExpectation = serde_json::from_value(value).ok()?;
            if !frozen.validate()
                || frozen.world_id != world_id
                || frozen.external_mail_id != inbound_mail_id
                || operation != "take"
            {
                return None;
            }
            Some(frozen)
        }
        None => None,
    };
    let usable = send_expectation.is_some()
        || match (origin_kind.as_str(), operation.as_str()) {
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
        fencing_version,
        sending_fencing_version,
        send_expectation,
        take_expectation,
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
    fencing_version: i64,
    expectation: Option<&DeliverySendExpectation>,
) -> Result<bool, auction_application::ApplicationError> {
    // 一次性尝试不是远端成功证明；仅明确中止的冲突可以重试，不重放未知提交。
    for attempt in 0..3 {
        match recovery::begin_discard_once(pool, delivery_id, fencing_version, expectation).await {
            Ok(changed) => return Ok(changed),
            Err(error) if error.retryable() && attempt < 2 => continue,
            Err(_) => return Err(storage_error("MoogleMail 草稿丢弃围栏提交未确认")),
        }
    }
    Err(storage_error("MoogleMail 草稿丢弃围栏重试已耗尽"))
}

#[cfg(feature = "postgres")]
enum RecoveryUpdate<'a> {
    Send(&'a str, &'a Value, i64, Option<&'a DeliverySendExpectation>),
    Action(&'a str, &'a Value, i64, &'a DeliveryTakeExpectation, &'a DeliverySendCredentialBinding),
    Requeue(i64, &'a str, i64, &'a DeliveryTakeExpectation, &'a DeliverySendCredentialBinding),
}

#[cfg(feature = "postgres")]
async fn bind_recovered_action(
    pool: &TrackedPool<Postgres>,
    delivery_id: Uuid,
    fencing_version: i64,
    external_mail_id: String,
    metadata: Value,
    expectation: &DeliveryTakeExpectation,
    observation: &DeliverySendCredentialBinding,
) -> Result<bool, auction_application::ApplicationError> {
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

#[cfg(feature = "postgres")]
async fn requeue_unproven(
    pool: &TrackedPool<Postgres>,
    delivery_id: Uuid,
    fencing_version: i64,
    retries: i64,
    reason: &str,
    expectation: &DeliveryTakeExpectation,
    observation: &DeliverySendCredentialBinding,
) -> Result<bool, auction_application::ApplicationError> {
    persist_transition(
        pool,
        delivery_id,
        RecoveryUpdate::Requeue(retries, reason, fencing_version, expectation, observation),
    )
    .await
}

#[cfg(feature = "postgres")]
async fn record_observation(
    pool: &TrackedPool<Postgres>,
    delivery_id: Uuid,
    fencing_version: i64,
    key: &str,
    observation: Value,
) -> Result<(), auction_application::ApplicationError> {
    // 只记录当前尝试的安全观察；旧响应不能附着到管理员重试后的新 fence。
    sqlx::query(
        "UPDATE public.deliveries
         SET result_metadata = COALESCE(result_metadata, '{}'::jsonb)
             || jsonb_build_object($2::text, $3::jsonb)
         WHERE id = $1 AND fencing_version=$4 AND status = 'manual_reconcile'",
    )
    .bind(delivery_id)
    .bind(key)
    .bind(observation)
    .bind(fencing_version)
    .execute(pool.raw())
    .await
    .map_err(|_| storage_error("MoogleMail 维护证据写入失败"))?;
    Ok(())
}

#[cfg(feature = "postgres")]
async fn bind_recovered_send(
    pool: &TrackedPool<Postgres>,
    delivery_id: Uuid,
    fencing_version: i64,
    external_mail_id: String,
    metadata: Value,
    expectation: Option<&DeliverySendExpectation>,
) -> Result<bool, auction_application::ApplicationError> {
    persist_transition(
        pool,
        delivery_id,
        RecoveryUpdate::Send(&external_mail_id, &metadata, fencing_version, expectation),
    )
    .await
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
    // 当前观测 generation→资金→recovery→credential→action→delivery；历史 generation 只作身份。
    match update {
        RecoveryUpdate::Send(_, metadata, _, Some(expectation)) => {
            recovery::lock_observation(&mut tx, expectation, metadata).await?;
        }
        RecoveryUpdate::Action(_, _, _, expectation, observation)
        | RecoveryUpdate::Requeue(_, _, _, expectation, observation) => {
            recovery::lock_take_observation(&mut tx, expectation, observation).await?;
        }
        RecoveryUpdate::Send(_, _, _, None) => {}
    }
    let account = funding::lock_funded_delivery_account(&mut tx, delivery_id).await?;
    let recovery_send = if matches!(update, RecoveryUpdate::Send(..)) {
        sqlx::query_scalar::<_, bool>(
            "SELECT origin_kind='settlement_recovery_action' FROM public.deliveries WHERE id=$1",
        )
        .bind(delivery_id)
        .fetch_one(&mut *tx)
        .await?
    } else {
        false
    };
    if recovery_send {
        super::recovery_commands::lock_recovery_delivery_retry_scope(&mut tx, delivery_id).await?;
    }
    if let RecoveryUpdate::Send(_, _, _, Some(expectation)) = update
        && !attempts::lock_credential(&mut tx, expectation).await?
    {
        return Err(RecoveryError::Rejected);
    }
    if let RecoveryUpdate::Action(_, _, _, expectation, _)
    | RecoveryUpdate::Requeue(_, _, _, expectation, _) = update
        && !attempts::lock_credential_binding(
            &mut tx,
            &expectation.world_id,
            expectation.credential.as_ref(),
        )
        .await?
    {
        return Err(RecoveryError::Rejected);
    }
    if recovery_send {
        super::recovery_commands::lock_recovery_delivery_retry_actions(&mut tx, delivery_id)
            .await?;
    }
    sqlx::query("SELECT id FROM public.deliveries WHERE id=$1 FOR UPDATE")
        .bind(delivery_id)
        .fetch_optional(&mut *tx)
        .await?;
    let taking_fencing_version = match update {
        RecoveryUpdate::Action(_, _, fencing_version, expectation, _)
        | RecoveryUpdate::Requeue(_, _, fencing_version, expectation, _) => {
            let Some((original_fencing, frozen)) =
                take_attempts::load_current(&mut tx, delivery_id, *fencing_version).await?
            else {
                tx.rollback().await?;
                return Ok(false);
            };
            if &frozen != *expectation {
                tx.rollback().await?;
                return Ok(false);
            }
            Some(original_fencing)
        }
        RecoveryUpdate::Send(..) => None,
    };
    let (origin_kind, origin_id, completed_at): (String, String, DateTime<Utc>) = match update {
        RecoveryUpdate::Requeue(retries, reason, fencing_version, _, _) => {
            let updated: Option<(String, DateTime<Utc>)> = sqlx::query_as(
                "UPDATE public.deliveries
                 SET status='pending', completed_at=NULL, failure_code=NULL, lease_token=NULL,
                     lease_expires_at=NULL, claimed_at=NULL, sending_at=NULL, updated_at=clock_timestamp(),
                     result_metadata=COALESCE(result_metadata, '{}'::jsonb)
                         || jsonb_build_object('sweep_retries',$2::bigint,
                             'sweep_requeued_at',clock_timestamp(),'sweep_requeue_reason',$3::text)
                 WHERE id=$1 AND status='manual_reconcile' AND origin_kind='mail_intake'
                   AND payload->>'operation'='take' AND external_reference IS NULL
                   AND COALESCE((result_metadata->>'sweep_retries')::bigint,0)=$2-1
                   AND fencing_version=$4
                 RETURNING origin_id,updated_at",
            )
            .bind(delivery_id)
            .bind(retries)
            .bind(reason)
            .bind(fencing_version)
            .fetch_optional(&mut *tx)
            .await?;
            let Some(row) = updated else {
                tx.rollback().await?;
                return Ok(false);
            };
            ("mail_intake".to_owned(), row.0, row.1)
        }
        RecoveryUpdate::Send(mid, metadata, fencing_version, expectation) => {
            let Some(row) = recovery::bind_send(
                &mut tx,
                delivery_id,
                *fencing_version,
                mid,
                metadata,
                *expectation,
            )
            .await?
            else {
                tx.rollback().await?;
                return Ok(false);
            };
            row
        }
        RecoveryUpdate::Action(mid, metadata, fencing_version, expectation, _) => {
            if *mid != expectation.external_mail_id
                || metadata.get("operation").and_then(Value::as_str) != Some("take")
                || metadata.get("proof").and_then(Value::as_str)
                    != Some("action_recovered_inbox_v1")
                || metadata.get("external_reference_proven").and_then(Value::as_bool) != Some(true)
                || metadata.get("recovered_mid").and_then(Value::as_str) != Some(*mid)
            {
                return Err(RecoveryError::Rejected);
            }
            let world = &expectation.world_id;
            let reference = format!("mooglemail:{world}:take:{mid}");
            let updated: Option<(String, DateTime<Utc>)> = sqlx::query_as(
                "UPDATE public.deliveries
                 SET external_reference=$2,status='succeeded',failure_code=NULL,
                     completed_at=clock_timestamp(),updated_at=clock_timestamp(),
                     lease_token=NULL,lease_expires_at=NULL,
                     result_metadata=COALESCE(result_metadata,'{}'::jsonb)||$3::jsonb
                 WHERE id=$1 AND status='manual_reconcile' AND origin_kind='mail_intake'
                   AND payload->>'operation'='take' AND external_reference IS NULL
                   AND sending_at IS NOT NULL AND COALESCE(payload->>'world','main')=$4
                   AND payload->>'external_mail_id'=$5
                   AND fencing_version=$6
                   AND NOT EXISTS (
                       SELECT 1 FROM public.deliveries other WHERE other.external_reference=$2)
                 RETURNING origin_id,completed_at",
            )
            .bind(delivery_id)
            .bind(&reference)
            .bind(*metadata)
            .bind(world)
            .bind(*mid)
            .bind(fencing_version)
            .fetch_optional(&mut *tx)
            .await?;
            let Some(row) = updated else {
                tx.rollback().await?;
                return Ok(false);
            };
            ("mail_intake".to_owned(), row.0, row.1)
        }
    };
    if matches!(update, RecoveryUpdate::Send(..))
        && let Some(account) = account
    {
        funding::finish_funded_delivery(
            &mut tx,
            &account,
            delivery_id,
            auction_application::FundingDeliveryOutcome::Sent,
        )
        .await?;
    }
    if origin_kind == "settlement" && matches!(update, RecoveryUpdate::Send(..)) {
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
    let resumed_action = if recovery_send {
        super::recovery_commands::resume_after_delivery_proof(&mut tx, delivery_id).await?
    } else {
        None
    };
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
                "resumed_recovery_action": resumed_action,
                "fencing_version": match update {
                    RecoveryUpdate::Send(_, _, fencing_version, _)
                    | RecoveryUpdate::Action(_, _, fencing_version, _, _)
                    | RecoveryUpdate::Requeue(_, _, fencing_version, _, _) => *fencing_version,
                },
                "taking_fencing_version": taking_fencing_version,
                "observation_credential": match update {
                    RecoveryUpdate::Action(_, _, _, _, observation)
                    | RecoveryUpdate::Requeue(_, _, _, _, observation) => Some(observation),
                    RecoveryUpdate::Send(..) => None,
                },
                "sending_fencing_version": match update {
                    RecoveryUpdate::Send(_, metadata, _, Some(_)) => {
                        metadata.get("sending_fencing_version").and_then(Value::as_i64)
                    }
                    _ => None,
                },
                "recovered_mid": match update {
                    RecoveryUpdate::Send(mid, _, _, _) | RecoveryUpdate::Action(mid, _, _, _, _) => Some(*mid),
                    _ => None,
                },
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
    Evidence(crate::MailEvidenceTransactionError),
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
impl From<crate::MailEvidenceTransactionError> for RecoveryError {
    fn from(error: crate::MailEvidenceTransactionError) -> Self {
        Self::Evidence(error)
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
            Self::Evidence(error) => error.is_serialization_conflict(),
            _ => false,
        }
    }
}

fn storage_error(message: &str) -> auction_application::ApplicationError {
    RepositoryError::Failed(message.to_owned()).into()
}
