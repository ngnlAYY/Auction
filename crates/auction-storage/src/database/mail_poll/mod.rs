//! MoogleMail Worker 的 PostgreSQL 持久化适配器。
//!
//! 邮箱状态、租约、入站事实和邮件命令账本由本模块拥有。外部 GET 不在数据库事务内执行；
//! Worker 只有在拿到 claim 后才调用 gateway，并在 commit 中原子写入事实与 cursor。

use auction_application::{
    AdminMailPollState, ApplicationError, ApplicationFuture, MAX_MAIL_POLL_BATCH_SIZE,
    MAX_MAIL_POLL_FAILURE_CODE_BYTES, MAX_MAIL_POLL_FAILURE_DETAIL_BYTES, MailPollBatch,
    MailPollClaim, MailPollStore, RepositoryError,
};
use chrono::{DateTime, Utc};

use super::DatabaseStore;

#[cfg(feature = "postgres")]
use auction_application::MailPollIntake;
#[cfg(feature = "postgres")]
use serde_json::Value;
#[cfg(feature = "postgres")]
use sqlx::{Postgres, Row, types::Json};
#[cfg(feature = "postgres")]
use uuid::Uuid;

impl MailPollStore for DatabaseStore {
    fn ensure_state(
        &self,
        mailbox_key: String,
        now: DateTime<Utc>,
    ) -> ApplicationFuture<'_, AdminMailPollState> {
        Box::pin(async move {
            if !valid_text(&mailbox_key, 256) || now.timestamp_nanos_opt().is_none() {
                return Err(storage_error("MoogleMail 邮箱初始化参数无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    ensure_state_postgres(store.pool(), &mailbox_key, now).await
                }
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(capability_error()),
            }
        })
    }

    fn claim_due(
        &self,
        mailbox_key: String,
        worker_id: String,
        now: DateTime<Utc>,
        lease_until: DateTime<Utc>,
        limit: u16,
        _force: bool,
    ) -> ApplicationFuture<'_, Vec<MailPollClaim>> {
        Box::pin(async move {
            if !valid_text(&mailbox_key, 256)
                || !valid_text(&worker_id, 256)
                || now.timestamp_nanos_opt().is_none()
                || lease_until <= now
                || limit == 0
                || limit > MAX_MAIL_POLL_BATCH_SIZE
            {
                return Err(storage_error("MoogleMail 轮询领取参数无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    claim_due_postgres(
                        store.pool(),
                        &mailbox_key,
                        &worker_id,
                        now,
                        lease_until,
                        limit,
                        _force,
                    )
                    .await
                }
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(capability_error()),
            }
        })
    }

    fn commit(
        &self,
        claim: MailPollClaim,
        batch: MailPollBatch,
    ) -> ApplicationFuture<'_, AdminMailPollState> {
        Box::pin(async move {
            if !claim.validate() || !batch.validate(&claim) {
                return Err(storage_error("MoogleMail 轮询提交参数无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => commit_postgres(store.pool(), &claim, &batch).await,
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(capability_error()),
            }
        })
    }

    fn renew(
        &self,
        claim: MailPollClaim,
        now: DateTime<Utc>,
        lease_until: DateTime<Utc>,
    ) -> ApplicationFuture<'_, MailPollClaim> {
        Box::pin(async move {
            if !claim.validate()
                || now.timestamp_nanos_opt().is_none()
                || lease_until <= now
                || lease_until <= claim.lease_until
            {
                return Err(storage_error("MoogleMail 轮询续租参数无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    renew_postgres(store.pool(), claim, now, lease_until).await
                }
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(capability_error()),
            }
        })
    }

    fn fail(
        &self,
        claim: MailPollClaim,
        retry_at: DateTime<Utc>,
        failure_code: String,
        safe_detail: String,
    ) -> ApplicationFuture<'_, AdminMailPollState> {
        Box::pin(async move {
            if !claim.validate()
                || retry_at.timestamp_nanos_opt().is_none()
                || !valid_text(&failure_code, MAX_MAIL_POLL_FAILURE_CODE_BYTES)
                || !valid_text(&safe_detail, MAX_MAIL_POLL_FAILURE_DETAIL_BYTES)
            {
                return Err(storage_error("MoogleMail 轮询失败记录参数无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    fail_postgres(store.pool(), &claim, retry_at, &failure_code, &safe_detail).await
                }
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(capability_error()),
            }
        })
    }
}

#[cfg(feature = "postgres")]
async fn renew_postgres(
    pool: &super::TrackedPool<Postgres>,
    mut claim: MailPollClaim,
    now: DateTime<Utc>,
    lease_until: DateTime<Utc>,
) -> Result<MailPollClaim, ApplicationError> {
    let updated = sqlx::query(
        "UPDATE public.mail_poll_state
         SET claim_expires_at = $4, updated_at = $3
         WHERE mailbox_key = $1 AND claim_token = $2 AND claim_owner = $5
               AND state_version = $6 AND paused = false
               AND claim_expires_at > $3 AND claim_expires_at < $4",
    )
    .bind(&claim.mailbox_key)
    .bind(claim.claim_token)
    .bind(now)
    .bind(lease_until)
    .bind(&claim.worker_id)
    .bind(claim.state_version)
    .execute(pool)
    .await
    .map_err(|_| storage_error("续租 PostgreSQL MoogleMail 轮询状态失败"))?;
    if updated.rows_affected() != 1 {
        return Err(storage_error("MoogleMail 轮询续租 fence 已失效"));
    }
    claim.lease_until = lease_until;
    Ok(claim)
}

#[cfg(feature = "postgres")]
async fn ensure_state_postgres(
    pool: &super::TrackedPool<Postgres>,
    mailbox_key: &str,
    now: DateTime<Utc>,
) -> Result<AdminMailPollState, ApplicationError> {
    sqlx::query(
        "INSERT INTO public.mail_poll_state
            (mailbox_key, cursor_value, last_poll_at, next_poll_at,
             consecutive_failures, paused, state_version, updated_at)
         VALUES ($1, NULL, $2, $2, 0, false, 1, $2)
         ON CONFLICT (mailbox_key) DO NOTHING",
    )
    .bind(mailbox_key)
    .bind(now)
    .execute(pool)
    .await
    .map_err(|_| storage_error("初始化 PostgreSQL MoogleMail 轮询状态失败"))?;
    let row = sqlx::query(
        "SELECT mailbox_key, cursor_value, last_poll_at, next_poll_at,
                consecutive_failures, paused, state_version, updated_at
         FROM public.mail_poll_state WHERE mailbox_key = $1",
    )
    .bind(mailbox_key)
    .fetch_one(pool)
    .await
    .map_err(|_| storage_error("读取 PostgreSQL MoogleMail 轮询状态失败"))?;
    map_state(&row)
}

#[cfg(feature = "postgres")]
async fn claim_due_postgres(
    pool: &super::TrackedPool<Postgres>,
    mailbox_key: &str,
    worker_id: &str,
    now: DateTime<Utc>,
    lease_until: DateTime<Utc>,
    limit: u16,
    force: bool,
) -> Result<Vec<MailPollClaim>, ApplicationError> {
    let mut transaction =
        pool.begin().await.map_err(|_| storage_error("开启 MoogleMail 领取事务失败"))?;
    let rows = sqlx::query(
        "SELECT mailbox_key, cursor_value, consecutive_failures, state_version
         FROM public.mail_poll_state
         WHERE mailbox_key = $1
           AND paused = false
           AND ($2 = true OR next_poll_at <= $3)
           AND (claim_expires_at IS NULL OR claim_expires_at <= $3)
           ORDER BY next_poll_at, mailbox_key
         LIMIT $4
         FOR UPDATE SKIP LOCKED",
    )
    .bind(mailbox_key)
    .bind(force)
    .bind(now)
    .bind(i64::from(limit))
    .fetch_all(&mut *transaction)
    .await
    .map_err(|_| storage_error("读取 due MoogleMail 邮箱失败"))?;

    let mut claims = Vec::with_capacity(rows.len());
    for row in rows {
        let mailbox_key: String = row
            .try_get("mailbox_key")
            .map_err(|_| storage_error("读取 MoogleMail 邮箱标识失败"))?;
        let cursor_value: Option<String> =
            row.try_get("cursor_value").map_err(|_| storage_error("读取 MoogleMail 游标失败"))?;
        let failures: i32 = row
            .try_get("consecutive_failures")
            .map_err(|_| storage_error("读取 MoogleMail 失败次数失败"))?;
        let state_version: i64 = row
            .try_get("state_version")
            .map_err(|_| storage_error("读取 MoogleMail 状态版本失败"))?;
        let attempt_count = failures
            .checked_add(1)
            .and_then(|value| u32::try_from(value).ok())
            .ok_or_else(|| storage_error("MoogleMail 失败次数超出安全范围"))?;
        let claim_token = Uuid::new_v4();
        let updated = sqlx::query(
            "UPDATE public.mail_poll_state
             SET claim_token = $2, claim_owner = $3, claim_expires_at = $4,
                 last_failure_code = NULL, last_failure_detail = NULL, last_failure_at = NULL,
                 state_version = state_version + 1, updated_at = CURRENT_TIMESTAMP
             WHERE mailbox_key = $1
               AND paused = false
               AND (claim_expires_at IS NULL OR claim_expires_at <= $5)",
        )
        .bind(&mailbox_key)
        .bind(claim_token)
        .bind(worker_id)
        .bind(lease_until)
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(|_| storage_error("写入 MoogleMail 租约失败"))?;
        if updated.rows_affected() != 1 {
            return Err(storage_error("MoogleMail 领取状态在事务内发生变化"));
        }
        claims.push(MailPollClaim {
            mailbox_key,
            worker_id: worker_id.to_owned(),
            claim_token,
            cursor_value,
            state_version: state_version
                .checked_add(1)
                .ok_or_else(|| storage_error("MoogleMail 状态版本溢出"))?,
            paused: false,
            attempt_count,
            lease_until,
        });
    }
    transaction.commit().await.map_err(|_| storage_error("提交 MoogleMail 领取事务失败"))?;
    Ok(claims)
}

#[cfg(feature = "postgres")]
async fn commit_postgres(
    pool: &super::TrackedPool<Postgres>,
    claim: &MailPollClaim,
    batch: &MailPollBatch,
) -> Result<AdminMailPollState, ApplicationError> {
    let mut transaction =
        pool.begin().await.map_err(|_| storage_error("开启 MoogleMail 提交事务失败"))?;
    let state = sqlx::query(
        "SELECT mailbox_key, cursor_value, last_poll_at, next_poll_at,
                consecutive_failures, paused, state_version, updated_at,
                claim_token, claim_owner, claim_expires_at
         FROM public.mail_poll_state
         WHERE mailbox_key = $1
         FOR UPDATE",
    )
    .bind(&claim.mailbox_key)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|_| storage_error("读取 MoogleMail 提交租约失败"))?;
    let Some(state) = state else {
        return rollback_error(transaction, "MoogleMail 提交邮箱状态不存在").await;
    };
    let stored_token: Option<Uuid> = state
        .try_get("claim_token")
        .map_err(|_| storage_error("读取 MoogleMail claim token 失败"))?;
    let stored_owner: Option<String> = state
        .try_get("claim_owner")
        .map_err(|_| storage_error("读取 MoogleMail claim owner 失败"))?;
    let expires_at: Option<DateTime<Utc>> = state
        .try_get("claim_expires_at")
        .map_err(|_| storage_error("读取 MoogleMail 租约过期时间失败"))?;
    let current_version: i64 = state
        .try_get("state_version")
        .map_err(|_| storage_error("读取 MoogleMail 提交版本失败"))?;
    if stored_token != Some(claim.claim_token)
        || stored_owner.as_deref() != Some(claim.worker_id.as_str())
        || current_version != claim.state_version
        || expires_at.is_none()
    {
        return rollback_error(transaction, "MoogleMail 提交租约已失效").await;
    }

    for item in &batch.items {
        insert_or_verify_intake(&mut transaction, &claim.mailbox_key, item).await?;
        insert_or_verify_mail_command(&mut transaction, &claim.mailbox_key, item).await?;
    }

    let cursor = batch.next_cursor.as_deref().or(claim.cursor_value.as_deref());
    let updated = sqlx::query(
        "UPDATE public.mail_poll_state
         SET cursor_value = $2,
             last_poll_at = GREATEST(last_poll_at, $3),
             next_poll_at = GREATEST(GREATEST(last_poll_at, $3), $4),
             consecutive_failures = 0,
             claim_token = NULL, claim_owner = NULL, claim_expires_at = NULL,
             last_failure_code = NULL, last_failure_detail = NULL, last_failure_at = NULL,
             state_version = state_version + 1, updated_at = $3
         WHERE mailbox_key = $1 AND claim_token = $5 AND claim_owner = $6
               AND state_version = $7 AND claim_expires_at > CURRENT_TIMESTAMP
         RETURNING mailbox_key, cursor_value, last_poll_at, next_poll_at,
                   consecutive_failures, paused, state_version, updated_at",
    )
    .bind(&claim.mailbox_key)
    .bind(cursor)
    .bind(batch.polled_at)
    .bind(batch.next_poll_at)
    .bind(claim.claim_token)
    .bind(&claim.worker_id)
    .bind(claim.state_version)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|_| storage_error("提交 MoogleMail cursor 失败"))?;
    let Some(updated) = updated else {
        return rollback_error(transaction, "MoogleMail 提交时租约已过期").await;
    };
    let result = map_state(&updated)?;
    transaction.commit().await.map_err(|_| storage_error("提交 MoogleMail 入站事务失败"))?;
    Ok(result)
}

#[cfg(feature = "postgres")]
async fn fail_postgres(
    pool: &super::TrackedPool<Postgres>,
    claim: &MailPollClaim,
    retry_at: DateTime<Utc>,
    failure_code: &str,
    safe_detail: &str,
) -> Result<AdminMailPollState, ApplicationError> {
    let row = sqlx::query(
        "UPDATE public.mail_poll_state
         SET next_poll_at = GREATEST(last_poll_at, $3),
             consecutive_failures = consecutive_failures + 1,
             claim_token = NULL, claim_owner = NULL, claim_expires_at = NULL,
             last_failure_code = $4, last_failure_detail = $5, last_failure_at = CURRENT_TIMESTAMP,
             state_version = state_version + 1, updated_at = CURRENT_TIMESTAMP
         WHERE mailbox_key = $1 AND claim_token = $2 AND claim_owner = $6
               AND state_version = $7 AND claim_expires_at > CURRENT_TIMESTAMP
         RETURNING mailbox_key, cursor_value, last_poll_at, next_poll_at,
                   consecutive_failures, paused, state_version, updated_at",
    )
    .bind(&claim.mailbox_key)
    .bind(claim.claim_token)
    .bind(retry_at)
    .bind(failure_code)
    .bind(safe_detail)
    .bind(&claim.worker_id)
    .bind(claim.state_version)
    .fetch_optional(pool)
    .await
    .map_err(|_| storage_error("写入 MoogleMail 失败退避状态失败"))?;
    let Some(row) = row else {
        return Err(storage_error("MoogleMail 失败记录租约已失效"));
    };
    map_state(&row)
}

#[cfg(feature = "postgres")]
async fn insert_or_verify_intake(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    mailbox_key: &str,
    item: &MailPollIntake,
) -> Result<(), ApplicationError> {
    sqlx::query(
        "INSERT INTO public.inbound_mail
            (id, mailbox_key, external_mail_id, world_id, mail_kind, content_digest,
             received_at, status, decision_code, decision_metadata, processed_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
         ON CONFLICT (mailbox_key, external_mail_id) DO NOTHING",
    )
    .bind(item.id)
    .bind(mailbox_key)
    .bind(&item.external_mail_id)
    .bind(&item.world_id)
    .bind(&item.mail_kind)
    .bind(item.content_digest.to_vec())
    .bind(item.received_at)
    .bind(&item.status)
    .bind(&item.decision_code)
    .bind(Json(item.decision_metadata.clone()))
    .bind(item.processed_at)
    .execute(&mut **transaction)
    .await
    .map_err(|_| storage_error("写入 MoogleMail 入站事实失败"))?;

    let row = sqlx::query(
        "SELECT id, world_id, mail_kind, content_digest, received_at, status,
                decision_code, decision_metadata, processed_at
         FROM public.inbound_mail
         WHERE mailbox_key = $1 AND external_mail_id = $2
         FOR UPDATE",
    )
    .bind(mailbox_key)
    .bind(&item.external_mail_id)
    .fetch_one(&mut **transaction)
    .await
    .map_err(|_| storage_error("校验 MoogleMail 入站幂等事实失败"))?;
    let stored_id: Uuid =
        row.try_get("id").map_err(|_| storage_error("读取 MoogleMail 入站 ID 失败"))?;
    let stored_world: String =
        row.try_get("world_id").map_err(|_| storage_error("读取 MoogleMail 入站世界失败"))?;
    let stored_kind: String =
        row.try_get("mail_kind").map_err(|_| storage_error("读取 MoogleMail 入站类型失败"))?;
    let stored_digest: Vec<u8> =
        row.try_get("content_digest").map_err(|_| storage_error("读取 MoogleMail 入站摘要失败"))?;
    let stored_received: DateTime<Utc> =
        row.try_get("received_at").map_err(|_| storage_error("读取 MoogleMail 入站时间失败"))?;
    let stored_status: String =
        row.try_get("status").map_err(|_| storage_error("读取 MoogleMail 入站状态失败"))?;
    let stored_code: String = row
        .try_get("decision_code")
        .map_err(|_| storage_error("读取 MoogleMail 入站决定码失败"))?;
    let stored_metadata: Json<Value> = row
        .try_get("decision_metadata")
        .map_err(|_| storage_error("读取 MoogleMail 入站 metadata 失败"))?;
    let stored_processed: DateTime<Utc> = row
        .try_get("processed_at")
        .map_err(|_| storage_error("读取 MoogleMail 入站处理时间失败"))?;
    if stored_id != item.id
        || stored_world != item.world_id
        || stored_kind != item.mail_kind
        || stored_digest != item.content_digest
        || stored_received != item.received_at
        || stored_status != item.status
        || stored_code != item.decision_code
        || stored_metadata.0 != item.decision_metadata
        || stored_processed != item.processed_at
    {
        return Err(storage_error("MoogleMail 外部邮件 ID 对应内容发生冲突"));
    }
    Ok(())
}

#[cfg(feature = "postgres")]
async fn insert_or_verify_mail_command(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    mailbox_key: &str,
    item: &MailPollIntake,
) -> Result<(), ApplicationError> {
    let command_id = Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("auction/mail-intake-command-v1/{mailbox_key}/{}", item.external_mail_id)
            .as_bytes(),
    );
    let result_payload = serde_json::json!({
        "mail_id": item.id,
        "status": item.status,
        "decision_code": item.decision_code,
    });
    sqlx::query(
        "INSERT INTO public.mail_commands
            (command_id, request_digest, mail_id, result_payload)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT (command_id) DO NOTHING",
    )
    .bind(command_id)
    .bind(item.content_digest.to_vec())
    .bind(item.id)
    .bind(Json(result_payload.clone()))
    .execute(&mut **transaction)
    .await
    .map_err(|_| storage_error("写入 MoogleMail 命令账本失败"))?;
    let row = sqlx::query(
        "SELECT request_digest, mail_id, result_payload
         FROM public.mail_commands WHERE command_id = $1",
    )
    .bind(command_id)
    .fetch_one(&mut **transaction)
    .await
    .map_err(|_| storage_error("读取 MoogleMail 命令账本失败"))?;
    let request_digest: Vec<u8> =
        row.try_get("request_digest").map_err(|_| storage_error("读取 MoogleMail 命令摘要失败"))?;
    let mail_id: Uuid =
        row.try_get("mail_id").map_err(|_| storage_error("读取 MoogleMail 命令邮件 ID 失败"))?;
    let stored_payload: Json<Value> =
        row.try_get("result_payload").map_err(|_| storage_error("读取 MoogleMail 命令结果失败"))?;
    if request_digest != item.content_digest
        || mail_id != item.id
        || stored_payload.0 != result_payload
    {
        return Err(storage_error("MoogleMail 命令账本发生请求冲突"));
    }
    Ok(())
}

#[cfg(feature = "postgres")]
fn map_state(row: &sqlx::postgres::PgRow) -> Result<AdminMailPollState, ApplicationError> {
    let state = AdminMailPollState {
        mailbox_key: row
            .try_get("mailbox_key")
            .map_err(|_| storage_error("读取 MoogleMail 邮箱标识失败"))?,
        cursor_value: row
            .try_get("cursor_value")
            .map_err(|_| storage_error("读取 MoogleMail 游标失败"))?,
        last_poll_at: row
            .try_get("last_poll_at")
            .map_err(|_| storage_error("读取 MoogleMail 上次轮询时间失败"))?,
        next_poll_at: row
            .try_get("next_poll_at")
            .map_err(|_| storage_error("读取 MoogleMail 下次轮询时间失败"))?,
        consecutive_failures: row
            .try_get("consecutive_failures")
            .map_err(|_| storage_error("读取 MoogleMail 失败次数失败"))?,
        paused: row.try_get("paused").map_err(|_| storage_error("读取 MoogleMail 暂停状态失败"))?,
        state_version: row
            .try_get("state_version")
            .map_err(|_| storage_error("读取 MoogleMail 状态版本失败"))?,
        updated_at: row
            .try_get("updated_at")
            .map_err(|_| storage_error("读取 MoogleMail 更新时间失败"))?,
    };
    if state.validate() {
        Ok(state)
    } else {
        Err(storage_error("MoogleMail 轮询状态安全投影无效"))
    }
}

#[cfg(feature = "postgres")]
async fn rollback_error(
    transaction: sqlx::Transaction<'_, sqlx::Postgres>,
    message: &str,
) -> Result<AdminMailPollState, ApplicationError> {
    let _ = transaction.rollback().await;
    Err(storage_error(message))
}

fn valid_text(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value == value.trim()
        && !value.chars().any(char::is_control)
}

fn storage_error(message: &str) -> ApplicationError {
    RepositoryError::Failed(message.to_owned()).into()
}

#[cfg(feature = "sqlite")]
fn capability_error() -> ApplicationError {
    ApplicationError::CapabilityUnavailable("MoogleMail Worker 轮询仅支持 PostgreSQL 完整档")
}
