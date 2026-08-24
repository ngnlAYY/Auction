//! MoogleMail 轮询状态初始化、续租和安全映射。

use auction_application::{AdminMailPollState, ApplicationError, MailPollClaim};
use chrono::{DateTime, Utc};
use sqlx::{Postgres, Row};

use super::storage_error;
use crate::database::TrackedPool;

pub(crate) async fn ensure(
    pool: &TrackedPool<Postgres>,
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
    map(&row)
}

pub(crate) async fn renew(
    pool: &TrackedPool<Postgres>,
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

pub(crate) fn map(row: &sqlx::postgres::PgRow) -> Result<AdminMailPollState, ApplicationError> {
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
