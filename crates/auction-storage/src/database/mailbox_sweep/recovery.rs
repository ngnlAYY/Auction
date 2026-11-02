//! 冻结账号与精确恢复门；旧尝试、过期 owner 与重复 MID 都不能冒充当前事实。

use auction_application::{
    DeliverySendCredentialBinding, DeliverySendExpectation, DeliveryTakeExpectation,
};
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use super::{RecoveryError, attempts};

/// 观测 owner 先于资金和业务锁；历史发送 generation 不要求等于重启后的当前 generation。
pub(super) async fn lock_observation(
    tx: &mut Transaction<'_, Postgres>,
    expectation: &DeliverySendExpectation,
    metadata: &Value,
) -> Result<(), RecoveryError> {
    let original = expectation.credential.as_ref().ok_or(RecoveryError::Rejected)?;
    let observed: DeliverySendCredentialBinding = serde_json::from_value(
        metadata.get("observation_credential").cloned().ok_or(RecoveryError::Rejected)?,
    )
    .map_err(|_| RecoveryError::Rejected)?;
    if !expectation.validate()
        || !observed.validate()
        || observed.credential_id != original.credential_id
        || observed.credential_version != original.credential_version
    {
        return Err(RecoveryError::Rejected);
    }
    crate::lock_mail_runtime_generation(
        tx,
        &auction_application::MailRuntimeBinding { generation: observed.runtime_generation },
    )
    .await?;
    Ok(())
}

/// Take 缺原绑定一律拒绝；同账号重启可换 generation，但本次观察必须属于当前 owner。
pub(super) async fn lock_take_observation(
    tx: &mut Transaction<'_, Postgres>,
    expectation: &DeliveryTakeExpectation,
    observed: &DeliverySendCredentialBinding,
) -> Result<(), RecoveryError> {
    let original = expectation.credential.as_ref().ok_or(RecoveryError::Rejected)?;
    if !expectation.validate()
        || !observed.validate()
        || observed.credential_id != original.credential_id
        || observed.credential_version != original.credential_version
    {
        return Err(RecoveryError::Rejected);
    }
    crate::lock_mail_runtime_generation(
        tx,
        &auction_application::MailRuntimeBinding { generation: observed.runtime_generation },
    )
    .await?;
    Ok(())
}

#[derive(sqlx::FromRow)]
struct RecoveryDelivery {
    origin_kind: String,
    origin_id: String,
    recipient: String,
    world_id: String,
    payload: Value,
    fencing_version: i64,
    status: String,
    external_reference: Option<String>,
    sending_at: Option<DateTime<Utc>>,
    completed_at: Option<DateTime<Utc>>,
}

/// 调用者已经按 runtime→funding→credential→delivery 锁序持锁；None 不算新增恢复。
pub(super) async fn bind_send(
    tx: &mut Transaction<'_, Postgres>,
    delivery_id: Uuid,
    fencing_version: i64,
    mid: &str,
    metadata: &Value,
    expected: Option<&DeliverySendExpectation>,
) -> Result<Option<(String, String, DateTime<Utc>)>, RecoveryError> {
    let Some(delivery): Option<RecoveryDelivery> = sqlx::query_as(
        "SELECT origin_kind,origin_id,recipient,COALESCE(payload->>'world','main') AS world_id,
                payload,fencing_version,status,external_reference,sending_at,completed_at
         FROM public.deliveries WHERE id=$1",
    )
    .bind(delivery_id)
    .fetch_optional(&mut **tx)
    .await?
    else {
        return Ok(None);
    };
    if delivery.fencing_version != fencing_version
        || delivery.payload.get("operation").and_then(Value::as_str) != Some("send")
        || metadata.get("operation").and_then(Value::as_str) != Some("send")
        || metadata.get("external_reference_proven").and_then(Value::as_bool) != Some(true)
        || metadata.get("sent_recovered").and_then(Value::as_bool) != Some(true)
        || metadata.get("recovered_mid").and_then(Value::as_str) != Some(mid)
        || metadata.get("recipient").and_then(Value::as_str) != Some(delivery.recipient.as_str())
    {
        return Ok(None);
    }
    let Some((sending_fencing_version, frozen)) =
        attempts::load_current(tx, delivery_id, fencing_version).await?
    else {
        return Ok(None);
    };
    if frozen.as_ref() != expected {
        return Ok(None);
    }
    if delivery.status != "manual_reconcile" || delivery.external_reference.is_some() {
        // 相同已提交绑定的重复观察是幂等无变化，不重复追加审计或扣款。
        return Ok(None);
    }
    let (Some(sending_at), Some(completed_at)) = (delivery.sending_at, delivery.completed_at)
    else {
        return Ok(None);
    };
    if let Some(expectation) = expected {
        if expectation.world_id != delivery.world_id
            || expectation.recipient != delivery.recipient
            || metadata.get("proof").and_then(Value::as_str) != Some("sent_recovered_frozen_v1")
            || metadata.get("sending_fencing_version").and_then(Value::as_i64)
                != Some(sending_fencing_version)
        {
            return Ok(None);
        }
        let sent_at = metadata
            .get("sent_at")
            .and_then(Value::as_str)
            .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
            .map(|value| value.with_timezone(&Utc))
            .ok_or(RecoveryError::Rejected)?;
        let now: DateTime<Utc> =
            sqlx::query_scalar("SELECT clock_timestamp()").fetch_one(&mut **tx).await?;
        if sent_at < sending_at
            || sent_at > completed_at
            || sent_at > now
            || ambiguous_attempt(
                tx,
                delivery_id,
                sending_fencing_version,
                expectation,
                Some(sent_at),
            )
            .await?
        {
            return Ok(None);
        }
    } else {
        // 没有新身份的旧记录仅保留原安全证明；绝不把当前 key 重建成历史正文。
        let legacy_proof = match delivery.origin_kind.as_str() {
            "settlement" => Some("sent_recovered_reference_v1"),
            "mail_intake"
                if delivery.payload.get("user_key_user_id").is_none_or(Value::is_null) =>
            {
                Some("sent_recovered_notice_v1")
            }
            _ => None,
        };
        if legacy_proof.is_none() || metadata.get("proof").and_then(Value::as_str) != legacy_proof {
            return Ok(None);
        }
    }
    if !attempts::bind_sent(
        tx,
        delivery_id,
        sending_fencing_version,
        &delivery.world_id,
        mid,
        expected.is_some(),
    )
    .await?
    {
        return Ok(None);
    }
    let reference = format!("mooglemail:{}:send:{mid}", delivery.world_id);
    let completed_at = sqlx::query_scalar(
        "UPDATE public.deliveries
         SET external_reference=$2,status='succeeded',failure_code=NULL,
             completed_at=clock_timestamp(),updated_at=clock_timestamp(),
             lease_token=NULL,lease_expires_at=NULL,
             result_metadata=COALESCE(result_metadata,'{}'::jsonb)||$3::jsonb
                 || CASE WHEN $5::jsonb IS NULL THEN '{}'::jsonb
                         ELSE jsonb_build_object('send_expectation',$5::jsonb) END
         WHERE id=$1 AND fencing_version=$4 AND status='manual_reconcile'
           AND external_reference IS NULL AND payload->>'operation'='send'
         RETURNING completed_at",
    )
    .bind(delivery_id)
    .bind(reference)
    .bind(metadata)
    .bind(fencing_version)
    .bind(expected.map(sqlx::types::Json))
    .fetch_one(&mut **tx)
    .await?;
    Ok(Some((delivery.origin_kind, delivery.origin_id, completed_at)))
}

/// 全库排除相同内容、同凭据且时间重叠的未绑定尝试，不能让批次前缀抢占不唯一的 MID。
/// 被管理员重试的旧尝试若已失去终态时窗，保守保留开放上界，不猜测远端未发送。
async fn ambiguous_attempt(
    tx: &mut Transaction<'_, Postgres>,
    delivery_id: Uuid,
    fencing_version: i64,
    expectation: &DeliverySendExpectation,
    sent_at: Option<DateTime<Utc>>,
) -> Result<bool, sqlx::Error> {
    // 与 Rust eq_ignore_ascii_case 同义，只折叠 ASCII；不能用受 locale 影响的 Unicode lower。
    sqlx::query_scalar(
        "SELECT EXISTS (
             SELECT 1 FROM public.delivery_send_attempts other
             JOIN public.deliveries delivery ON delivery.id=other.delivery_id
             LEFT JOIN public.delivery_sent_bindings bound
                 ON bound.delivery_id=other.delivery_id AND bound.fencing_version=other.fencing_version
             WHERE (other.delivery_id,other.fencing_version)<>($1,$2)
               AND bound.delivery_id IS NULL AND other.world_id=$3
               AND translate(other.expectation->>'recipient','ABCDEFGHIJKLMNOPQRSTUVWXYZ','abcdefghijklmnopqrstuvwxyz') COLLATE \"C\"
                   = translate($4::jsonb->>'recipient','ABCDEFGHIJKLMNOPQRSTUVWXYZ','abcdefghijklmnopqrstuvwxyz') COLLATE \"C\"
               AND other.expectation->'body_digest'=$4::jsonb->'body_digest'
               AND other.expectation-ARRAY['credential','recipient']::text[]
                   = $4::jsonb-ARRAY['credential','recipient']::text[]
               AND other.expectation->'credential'->>'credential_id'
                   IS NOT DISTINCT FROM $4::jsonb->'credential'->>'credential_id'
               AND other.expectation->'credential'->>'credential_version'
                   IS NOT DISTINCT FROM $4::jsonb->'credential'->>'credential_version'
               AND ($5::timestamptz IS NULL OR (
                   other.sending_at<=$5
                   AND (delivery.sending_at IS DISTINCT FROM other.sending_at
                        OR delivery.command_id<>other.command_id
                        OR delivery.request_digest<>other.request_digest
                        OR delivery.completed_at IS NULL OR delivery.completed_at>=$5)
               ))
         )",
    )
    .bind(delivery_id)
    .bind(fencing_version)
    .bind(&expectation.world_id)
    .bind(sqlx::types::Json(expectation))
    .bind(sent_at)
    .fetch_one(&mut **tx)
    .await
}

/// 自动恢复和人工 Sent 证明共用原履约复位；仅恢复正常对账，不满足付款或重发资产。
pub(in crate::database) async fn resume_settlement_send(
    tx: &mut Transaction<'_, Postgres>,
    obligation_id: Uuid,
    delivery_id: Uuid,
) -> Result<(), sqlx::Error> {
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
    .execute(&mut **tx)
    .await?;
    if enqueued.rows_affected() != 1 {
        return Err(sqlx::Error::RowNotFound);
    }
    Ok(())
}

/// 草稿没有发送时间可消歧；全库存在相同未绑定身份时保守拒绝一次性 discard 授权。
pub(super) async fn begin_discard_once(
    pool: &super::TrackedPool<Postgres>,
    delivery_id: Uuid,
    fencing_version: i64,
    expected: Option<&DeliverySendExpectation>,
) -> Result<bool, RecoveryError> {
    let mut tx = pool.begin_with("BEGIN ISOLATION LEVEL SERIALIZABLE").await?;
    sqlx::query("SELECT id FROM public.deliveries WHERE id=$1 FOR UPDATE")
        .bind(delivery_id)
        .fetch_optional(&mut *tx)
        .await?;
    let Some((sending_fencing_version, frozen)) =
        attempts::load_current(&mut tx, delivery_id, fencing_version).await?
    else {
        tx.rollback().await?;
        return Ok(false);
    };
    if frozen.as_ref() != expected {
        tx.rollback().await?;
        return Ok(false);
    }
    if let Some(expectation) = expected
        && (expectation.credential.is_none()
            || ambiguous_attempt(&mut tx, delivery_id, sending_fencing_version, expectation, None)
                .await?)
    {
        tx.rollback().await?;
        return Ok(false);
    }
    let result = sqlx::query(
        "UPDATE public.deliveries
         SET result_metadata=COALESCE(result_metadata,'{}'::jsonb)
             || jsonb_build_object('compose_discard_attempt',
                 jsonb_build_object('started_at',clock_timestamp(),'fencing_version',$2::bigint))
         WHERE id=$1 AND fencing_version=$2 AND status='manual_reconcile'
           AND external_reference IS NULL AND payload->>'operation'='send'
           AND sending_at IS NOT NULL AND completed_at IS NOT NULL
           AND ($3 OR (origin_kind IN ('settlement','mail_intake')
               AND COALESCE(payload->>'world','main')='main'
               AND COALESCE(payload->'variables'->>'reference','')<>''))
           AND NOT EXISTS (SELECT 1 FROM public.delivery_sent_bindings
               WHERE delivery_id=$1 AND fencing_version=$4)
           AND NOT (COALESCE(result_metadata,'{}'::jsonb) ? 'compose_discard_attempt')
           AND NOT (COALESCE(result_metadata,'{}'::jsonb) ? 'compose_discard')",
    )
    .bind(delivery_id)
    .bind(fencing_version)
    .bind(expected.is_some())
    .bind(sending_fencing_version)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(result.rows_affected() == 1)
}
