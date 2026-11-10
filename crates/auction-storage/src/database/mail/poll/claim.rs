//! MoogleMail Worker 的 claim、commit 和失败退避事务。

use crate::database::storage_error_from;

use auction_application::admin::AdminMailPollState;
use auction_application::core::ApplicationError;
use auction_application::mail::{MailPollBatch, MailPollClaim};
use chrono::{DateTime, Utc};
use sqlx::{Postgres, Row};
use uuid::Uuid;

use super::inbound::{insert_or_verify_intake, insert_or_verify_mail_command};
use super::state::map;
use super::storage_error;
use crate::database::TrackedPool;

pub(crate) async fn claim(
    pool: &TrackedPool<Postgres>,
    mailbox_key: &str,
    worker_id: &str,
    now: DateTime<Utc>,
    lease_until: DateTime<Utc>,
    limit: u16,
    force: bool,
) -> Result<Vec<MailPollClaim>, ApplicationError> {
    // force 只允许本次手动唤醒越过 paused/next_poll_at，租约、owner 和版本 fence 仍照常生效。
    let mut transaction = pool
        .begin()
        .await
        .map_err(|error| storage_error_from(error, "开启 MoogleMail 领取事务失败"))?;
    let rows = sqlx::query(
        "SELECT mailbox_key, cursor_value, consecutive_failures, paused, state_version
         FROM public.mail_poll_state
         WHERE mailbox_key = $1
           AND ($2 = true OR paused = false)
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
    .map_err(|error| storage_error_from(error, "读取 due MoogleMail 邮箱失败"))?;

    let mut claims = Vec::with_capacity(rows.len());
    for row in rows {
        let mailbox_key: String = row
            .try_get("mailbox_key")
            .map_err(|error| storage_error_from(error, "读取 MoogleMail 邮箱标识失败"))?;
        let cursor_value: Option<String> = row
            .try_get("cursor_value")
            .map_err(|error| storage_error_from(error, "读取 MoogleMail 游标失败"))?;
        let failures: i32 = row
            .try_get("consecutive_failures")
            .map_err(|error| storage_error_from(error, "读取 MoogleMail 失败次数失败"))?;
        let paused: bool = row
            .try_get("paused")
            .map_err(|error| storage_error_from(error, "读取 MoogleMail 暂停状态失败"))?;
        let state_version: i64 = row
            .try_get("state_version")
            .map_err(|error| storage_error_from(error, "读取 MoogleMail 状态版本失败"))?;
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
               AND ($6 = true OR paused = false)
               AND (claim_expires_at IS NULL OR claim_expires_at <= $5)",
        )
        .bind(&mailbox_key)
        .bind(claim_token)
        .bind(worker_id)
        .bind(lease_until)
        .bind(now)
        .bind(force)
        .execute(&mut *transaction)
        .await
        .map_err(|error| storage_error_from(error, "写入 MoogleMail 租约失败"))?;
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
            paused,
            attempt_count,
            lease_until,
        });
    }
    transaction
        .commit()
        .await
        .map_err(|error| storage_error_from(error, "提交 MoogleMail 领取事务失败"))?;
    Ok(claims)
}

pub(crate) async fn commit(
    pool: &TrackedPool<Postgres>,
    claim: &MailPollClaim,
    batch: &MailPollBatch,
    failure: Option<(&str, &str)>,
) -> Result<AdminMailPollState, ApplicationError> {
    let mut transaction = pool
        .begin()
        .await
        .map_err(|error| storage_error_from(error, "开启 MoogleMail 提交事务失败"))?;
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
    .map_err(|error| storage_error_from(error, "读取 MoogleMail 提交租约失败"))?;
    let Some(state) = state else {
        return rollback_error(transaction, "MoogleMail 提交邮箱状态不存在").await;
    };
    let stored_token: Option<Uuid> = state
        .try_get("claim_token")
        .map_err(|error| storage_error_from(error, "读取 MoogleMail claim token 失败"))?;
    let stored_owner: Option<String> = state
        .try_get("claim_owner")
        .map_err(|error| storage_error_from(error, "读取 MoogleMail claim owner 失败"))?;
    let expires_at: Option<DateTime<Utc>> = state
        .try_get("claim_expires_at")
        .map_err(|error| storage_error_from(error, "读取 MoogleMail 租约过期时间失败"))?;
    let current_version: i64 = state
        .try_get("state_version")
        .map_err(|error| storage_error_from(error, "读取 MoogleMail 提交版本失败"))?;
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

    super::reads::complete_prefix(
        &mut transaction,
        &claim.mailbox_key,
        batch.next_cursor.as_deref(),
    )
    .await?;

    let cursor = batch.next_cursor.as_deref().or(claim.cursor_value.as_deref());
    // 入站写入及锁等待可能跨越租约截止；最后的 cursor CAS 必须使用实时数据库时间。
    let updated = sqlx::query(
        "UPDATE public.mail_poll_state
         SET cursor_value = $2,
             last_poll_at = GREATEST(last_poll_at, $3),
             next_poll_at = GREATEST(GREATEST(last_poll_at, $3), $4),
             consecutive_failures = CASE WHEN $8::text IS NULL THEN 0 ELSE consecutive_failures + 1 END,
             claim_token = NULL, claim_owner = NULL, claim_expires_at = NULL,
             last_failure_code = $8, last_failure_detail = $9,
             last_failure_at = CASE WHEN $8::text IS NULL THEN NULL ELSE CURRENT_TIMESTAMP END,
             state_version = state_version + 1, updated_at = $3
         WHERE mailbox_key = $1 AND claim_token = $5 AND claim_owner = $6
               AND state_version = $7 AND claim_expires_at > clock_timestamp()
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
    .bind(failure.map(|(code, _)| code))
    .bind(failure.map(|(_, detail)| detail))
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|error| storage_error_from(error, "提交 MoogleMail cursor 失败"))?;
    let Some(updated) = updated else {
        return rollback_error(transaction, "MoogleMail 提交时租约已过期").await;
    };
    let result = map(&updated)?;
    transaction
        .commit()
        .await
        .map_err(|error| storage_error_from(error, "提交 MoogleMail 入站事务失败"))?;
    Ok(result)
}

pub(crate) async fn fail(
    pool: &TrackedPool<Postgres>,
    claim: &MailPollClaim,
    retry_at: DateTime<Utc>,
    failure_code: &str,
    safe_detail: &str,
) -> Result<AdminMailPollState, ApplicationError> {
    let mut transaction = pool
        .begin()
        .await
        .map_err(|error| storage_error_from(error, "开启 MoogleMail 失败退避事务失败"))?;
    super::state::lock_mailbox(&mut transaction, &claim.mailbox_key).await?;
    let row = sqlx::query(
        "UPDATE public.mail_poll_state
         SET last_poll_at = GREATEST(last_poll_at, CURRENT_TIMESTAMP),
             next_poll_at = GREATEST(GREATEST(last_poll_at, CURRENT_TIMESTAMP), $3),
             consecutive_failures = consecutive_failures + 1,
             claim_token = NULL, claim_owner = NULL, claim_expires_at = NULL,
             last_failure_code = $4, last_failure_detail = $5, last_failure_at = CURRENT_TIMESTAMP,
             state_version = state_version + 1, updated_at = CURRENT_TIMESTAMP
         WHERE mailbox_key = $1 AND claim_token = $2 AND claim_owner = $6
               AND state_version = $7 AND claim_expires_at > clock_timestamp()
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
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|error| storage_error_from(error, "写入 MoogleMail 失败退避状态失败"))?;
    let Some(row) = row else {
        return Err(storage_error("MoogleMail 失败记录租约已失效"));
    };
    let result = map(&row)?;
    transaction
        .commit()
        .await
        .map_err(|error| storage_error_from(error, "提交 MoogleMail 失败退避事务失败"))?;
    Ok(result)
}

async fn rollback_error(
    transaction: crate::database::TrackedTransaction<'_, sqlx::Postgres>,
    message: &str,
) -> Result<AdminMailPollState, ApplicationError> {
    let _ = transaction.rollback().await;
    Err(storage_error(message))
}
