//! 邮件证据会话与世界就绪的 PostgreSQL 写端。
//!
//! 不变量 5、6：同一顶层事务先写会话再续期世界就绪；时间只取数据库时钟，任何不一致或
//! 缺失都失败关闭，不返回本地伪造的有效期。

use auction_application::consignment::{
    ConsignmentError, MailEvidenceReadinessRecord, MailEvidenceReadinessStore, MailReadinessReceipt,
};
use auction_application::core::TransactionFuture;
use auction_domain::valid_text;
use auction_time::Timestamp;
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use crate::database::DatabaseStore;

const MAX_VERSION: u64 = 9_007_199_254_740_991;
const MAX_READINESS_TTL_SECONDS: u32 = 86_400;
const MAX_ALLOWED_SENDERS: usize = 1_000;
const MAX_VALIDATOR_VERSIONS: usize = 32;

impl MailEvidenceReadinessStore for DatabaseStore {
    fn record_mail_readiness(
        &self,
        record: MailEvidenceReadinessRecord,
    ) -> TransactionFuture<'_, MailReadinessReceipt, ConsignmentError> {
        Box::pin(async move {
            if !valid_record(&record) {
                return Err(ConsignmentError::InvalidDepositRequest);
            }
            let mut transaction = self.pool().begin_with("BEGIN").await.map_err(unavailable)?;
            let receipt = record_in_tx(&mut transaction, &record).await.map_err(unavailable)?;
            // 不变量 5：未知提交只让心跳下一轮重试，不返回伪造就绪。
            transaction.commit().await.map_err(outcome_unknown)?;
            Ok(receipt)
        })
    }
}

async fn record_in_tx(
    connection: &mut PgConnection,
    record: &MailEvidenceReadinessRecord,
) -> Result<MailReadinessReceipt, sqlx::Error> {
    let session = &record.session;
    let world = session.world.as_str();
    let credential_version =
        i64::try_from(session.credential_version).map_err(version_out_of_range)?;
    let runtime_generation =
        i64::try_from(session.runtime_generation).map_err(version_out_of_range)?;
    let service_epoch = i64::try_from(record.service_epoch).map_err(version_out_of_range)?;
    sqlx::query(
        "INSERT INTO public.mail_evidence_sessions
            (id, bot_uid, world_id, player_name, credential_id, credential_version,
             runtime_generation, observed_at, protocol_version, binding_digest)
         VALUES ($1, $2, $3, $4, $5, $6, $7, clock_timestamp(), $8, $9)
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(session.session_id)
    .bind(&session.bot_uid)
    .bind(world)
    .bind(&session.player_name)
    .bind(session.credential_id)
    .bind(credential_version)
    .bind(runtime_generation)
    .bind(&session.protocol_version)
    .bind(session.binding_digest.as_slice())
    .execute(&mut *connection)
    .await?;

    let stored = sqlx::query(
        "SELECT bot_uid, world_id, player_name, credential_id, credential_version,
                runtime_generation, protocol_version, binding_digest
         FROM public.mail_evidence_sessions WHERE id = $1",
    )
    .bind(session.session_id)
    .fetch_optional(&mut *connection)
    .await?
    .ok_or(sqlx::Error::RowNotFound)?;
    let stored_binding: &[u8] = stored.try_get("binding_digest")?;
    if stored.try_get::<String, _>("bot_uid")? != session.bot_uid
        || stored.try_get::<String, _>("world_id")? != world
        || stored.try_get::<String, _>("player_name")? != session.player_name
        || stored.try_get::<Uuid, _>("credential_id")? != session.credential_id
        || stored.try_get::<i64, _>("credential_version")? != credential_version
        || stored.try_get::<i64, _>("runtime_generation")? != runtime_generation
        || stored.try_get::<String, _>("protocol_version")? != session.protocol_version
        || stored_binding != session.binding_digest.as_slice()
    {
        return Err(sqlx::Error::RowNotFound);
    }

    // 会话行只写一次；每次心跳都必须重新证明凭据仍是当前 active 版本（与插入触发器同一条件）。
    let credential_current: bool = sqlx::query_scalar(
        "SELECT EXISTS (
             SELECT 1 FROM public.credential_keys
             WHERE id = $1 AND credential_kind = 'mail_cookie' AND scope = $2
               AND status = 'active' AND state_version = $3
         )",
    )
    .bind(session.credential_id)
    .bind(world)
    .bind(credential_version)
    .fetch_one(&mut *connection)
    .await?;
    if !credential_current {
        return Err(sqlx::Error::RowNotFound);
    }

    let policy = &record.policy;
    let row = sqlx::query(
        "WITH wall_clock AS (SELECT clock_timestamp() AS now)
         INSERT INTO public.consignment_mail_readiness
            (world_id, session_id, service_epoch, policy_digest, allowlist_mode,
             allowed_senders, validator_versions, detail_ready, take_ready, return_ready,
             checked_at, valid_until, version)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
                 (SELECT now FROM wall_clock),
                 (SELECT now FROM wall_clock) + make_interval(secs => $11::double precision), 1)
         ON CONFLICT (world_id) DO UPDATE SET
             session_id = EXCLUDED.session_id,
             service_epoch = EXCLUDED.service_epoch,
             policy_digest = EXCLUDED.policy_digest,
             allowlist_mode = EXCLUDED.allowlist_mode,
             allowed_senders = EXCLUDED.allowed_senders,
             validator_versions = EXCLUDED.validator_versions,
             detail_ready = EXCLUDED.detail_ready,
             take_ready = EXCLUDED.take_ready,
             return_ready = EXCLUDED.return_ready,
             checked_at = GREATEST(EXCLUDED.checked_at, consignment_mail_readiness.checked_at),
             valid_until = GREATEST(EXCLUDED.checked_at,
                                    consignment_mail_readiness.checked_at)
                           + make_interval(secs => $11::double precision),
             version = consignment_mail_readiness.version + 1
         RETURNING version, checked_at, valid_until",
    )
    .bind(world)
    .bind(session.session_id)
    .bind(service_epoch)
    .bind(policy.policy_digest.as_slice())
    .bind(&policy.allowlist_mode)
    .bind(&policy.allowed_senders)
    .bind(&policy.validator_versions)
    .bind(policy.detail_ready)
    .bind(policy.take_ready)
    .bind(policy.return_ready)
    .bind(i64::from(record.valid_for_seconds))
    .fetch_one(&mut *connection)
    .await?;
    let version = u64::try_from(row.try_get::<i64, _>("version")?).map_err(version_out_of_range)?;
    Ok(MailReadinessReceipt {
        version,
        checked_at: row.try_get::<Timestamp, _>("checked_at")?,
        valid_until: row.try_get::<Timestamp, _>("valid_until")?,
    })
}

fn valid_record(record: &MailEvidenceReadinessRecord) -> bool {
    let session = &record.session;
    let policy = &record.policy;
    !session.session_id.is_nil()
        && matches!(session.world.as_str(), "main" | "isekai")
        && valid_bot_uid(&session.bot_uid)
        && valid_text(&session.player_name, 256)
        && !session.credential_id.is_nil()
        && (1..=MAX_VERSION).contains(&session.credential_version)
        && (1..=MAX_VERSION).contains(&session.runtime_generation)
        && valid_text(&session.protocol_version, 64)
        && session.binding_digest != [0; 32]
        && policy.policy_digest != [0; 32]
        && matches!(policy.allowlist_mode.as_str(), "enforced" | "disabled")
        && policy.allowed_senders.len() <= MAX_ALLOWED_SENDERS
        && policy.allowed_senders.iter().all(|sender| valid_text(sender, 128))
        && policy.validator_versions.len() <= MAX_VALIDATOR_VERSIONS
        && policy.validator_versions.iter().all(|version| valid_text(version, 64))
        && (1..=MAX_VERSION).contains(&record.service_epoch)
        && (1..=MAX_READINESS_TTL_SECONDS).contains(&record.valid_for_seconds)
}

fn valid_bot_uid(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 32
        && value.bytes().all(|byte| byte.is_ascii_digit())
        && value.as_bytes()[0] != b'0'
}

fn unavailable(error: impl std::fmt::Display) -> ConsignmentError {
    tracing::warn!(error = %error, "寄售收件就绪写入失败");
    ConsignmentError::Unavailable
}

fn outcome_unknown(error: sqlx::Error) -> ConsignmentError {
    tracing::warn!(error = %error, "寄售收件就绪提交结果未知");
    ConsignmentError::OutcomeUnknown
}

fn version_out_of_range(error: std::num::TryFromIntError) -> sqlx::Error {
    tracing::warn!(error = %error, "寄售收件就绪版本越界");
    sqlx::Error::RowNotFound
}
