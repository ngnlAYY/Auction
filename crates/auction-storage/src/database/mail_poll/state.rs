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
    scheduled_next_at: DateTime<Utc>,
    retry_maximum_seconds: i64,
) -> Result<AdminMailPollState, ApplicationError> {
    sqlx::query(
        "INSERT INTO public.mail_poll_state
            (mailbox_key, cursor_value, last_poll_at, next_poll_at,
             consecutive_failures, paused, state_version, updated_at)
         VALUES ($1, NULL, $2, $3, 0, false, 1, $2)
         ON CONFLICT (mailbox_key) DO NOTHING",
    )
    .bind(mailbox_key)
    .bind(now)
    .bind(scheduled_next_at)
    .execute(pool)
    .await
    .map_err(|_| storage_error("初始化 PostgreSQL MoogleMail 轮询状态失败"))?;

    // 旧配置可能把持久化水位留在很远的未来。启动时只允许把它向当前 cron 收敛；暂停、
    // 活动 claim 和失败退避仍是权威状态，不能因为进程重启而被跳过。
    sqlx::query(
        "UPDATE public.mail_poll_state
         SET next_poll_at = CASE
                 WHEN consecutive_failures > 0 AND last_failure_at IS NOT NULL
                     THEN GREATEST($3, last_failure_at + ($4 * interval '1 second'))
                 ELSE $3
             END,
             state_version = state_version + 1,
             updated_at = $2
         WHERE mailbox_key = $1
           AND paused = false
           AND (claim_expires_at IS NULL OR claim_expires_at <= $2)
           AND next_poll_at > CASE
                 WHEN consecutive_failures > 0 AND last_failure_at IS NOT NULL
                     THEN GREATEST($3, last_failure_at + ($4 * interval '1 second'))
                 ELSE $3
             END",
    )
    .bind(mailbox_key)
    .bind(now)
    .bind(scheduled_next_at)
    .bind(retry_maximum_seconds)
    .execute(pool)
    .await
    .map_err(|_| storage_error("收敛 PostgreSQL MoogleMail 调度水位失败"))?;

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
    let mut transaction =
        pool.begin().await.map_err(|_| storage_error("开启 MoogleMail 续租事务失败"))?;
    lock_mailbox(&mut transaction, &claim.mailbox_key).await?;
    // 手动触发可以持有 paused 快照；续租仍必须匹配该快照，避免改变自动调度语义。
    let updated = sqlx::query(
        "UPDATE public.mail_poll_state
         SET claim_expires_at = $4, updated_at = $3
         WHERE mailbox_key = $1 AND claim_token = $2 AND claim_owner = $5
               AND state_version = $6 AND paused = $7
               AND claim_expires_at > $3 AND claim_expires_at < $4
               AND claim_expires_at > clock_timestamp() AND $4 > clock_timestamp()",
    )
    .bind(&claim.mailbox_key)
    .bind(claim.claim_token)
    .bind(now)
    .bind(lease_until)
    .bind(&claim.worker_id)
    .bind(claim.state_version)
    .bind(claim.paused)
    .execute(&mut *transaction)
    .await
    .map_err(|_| storage_error("续租 PostgreSQL MoogleMail 轮询状态失败"))?;
    if updated.rows_affected() != 1 {
        return Err(storage_error("MoogleMail 轮询续租 fence 已失效"));
    }
    transaction.commit().await.map_err(|_| storage_error("提交 MoogleMail 续租事务失败"))?;
    claim.lease_until = lease_until;
    Ok(claim)
}

/// 先取得邮箱行锁，后续写入再检查数据库实时时钟，不能用等待锁前的时间复活或完成过期租约。
pub(super) async fn lock_mailbox(
    transaction: &mut sqlx::Transaction<'_, Postgres>,
    mailbox_key: &str,
) -> Result<(), ApplicationError> {
    sqlx::query("SELECT mailbox_key FROM public.mail_poll_state WHERE mailbox_key = $1 FOR UPDATE")
        .bind(mailbox_key)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(|_| storage_error("锁定 MoogleMail 邮箱租约失败"))?;
    Ok(())
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
