//! 详情读取恢复队列：短事务锁定邮箱租约，GET 前提交摘要；入站提交原子删除完成前缀。

use auction_application::{ApplicationError, MailPollClaim, MailPollReadSummary};
use sqlx::{Postgres, Row, Transaction};

use super::storage_error;
use crate::database::TrackedPool;

pub(super) async fn pending(
    pool: &TrackedPool<Postgres>,
    claim: &MailPollClaim,
    world_id: &str,
    limit: u16,
) -> Result<Vec<MailPollReadSummary>, ApplicationError> {
    let mut transaction =
        pool.begin().await.map_err(|_| storage_error("开启 MoogleMail 待读事务失败"))?;
    lock_claim(&mut transaction, claim, world_id).await?;
    let rows = sqlx::query(
        "SELECT external_mail_id, world_id, sender, subject, received_at
         FROM public.mail_poll_pending_reads
         WHERE mailbox_key = $1 AND world_id = $2
         ORDER BY read_sequence LIMIT $3",
    )
    .bind(&claim.mailbox_key)
    .bind(world_id)
    .bind(i64::from(limit))
    .fetch_all(&mut *transaction)
    .await
    .map_err(|_| storage_error("读取 MoogleMail 待读队列失败"))?;
    let summaries = rows
        .into_iter()
        .map(|row| {
            let summary = MailPollReadSummary {
                external_mail_id: row.try_get("external_mail_id")?,
                world_id: row.try_get("world_id")?,
                sender: row.try_get("sender")?,
                subject: row.try_get("subject")?,
                received_at: row.try_get("received_at")?,
            };
            Ok::<_, sqlx::Error>(summary)
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| storage_error("解码 MoogleMail 待读身份失败"))?;
    if summaries.iter().any(|summary| !summary.validate()) {
        return Err(storage_error("MoogleMail 待读身份无效"));
    }
    check_live_claim(&mut transaction, claim).await?;
    transaction.commit().await.map_err(|_| storage_error("提交 MoogleMail 待读事务失败"))?;
    Ok(summaries)
}

pub(super) async fn stage(
    pool: &TrackedPool<Postgres>,
    claim: &MailPollClaim,
    world_id: &str,
    summaries: &[MailPollReadSummary],
) -> Result<(), ApplicationError> {
    let mut transaction =
        pool.begin().await.map_err(|_| storage_error("开启 MoogleMail 待读登记事务失败"))?;
    lock_claim(&mut transaction, claim, world_id).await?;
    for summary in summaries {
        // 冲突时只校验原身份，绝不覆盖第一次 GET 前已提交的恢复依据。
        let verified = sqlx::query(
            "INSERT INTO public.mail_poll_pending_reads
                (mailbox_key, world_id, external_mail_id, sender, subject, received_at)
             VALUES ($1, $2, $3, $4, $5, $6)
             ON CONFLICT (mailbox_key, external_mail_id) DO UPDATE
             SET external_mail_id = EXCLUDED.external_mail_id
             WHERE mail_poll_pending_reads.world_id = EXCLUDED.world_id
               AND mail_poll_pending_reads.sender = EXCLUDED.sender
               AND mail_poll_pending_reads.subject = EXCLUDED.subject
               AND mail_poll_pending_reads.received_at = EXCLUDED.received_at",
        )
        .bind(&claim.mailbox_key)
        .bind(world_id)
        .bind(&summary.external_mail_id)
        .bind(&summary.sender)
        .bind(&summary.subject)
        .bind(summary.received_at)
        .execute(&mut *transaction)
        .await
        .map_err(|_| storage_error("登记 MoogleMail 待读身份失败"))?;
        if verified.rows_affected() != 1 {
            return Err(storage_error("MoogleMail 待读身份冲突"));
        }
    }
    check_live_claim(&mut transaction, claim).await?;
    transaction.commit().await.map_err(|_| storage_error("提交 MoogleMail 待读登记失败"))
}

async fn lock_claim(
    transaction: &mut Transaction<'_, Postgres>,
    claim: &MailPollClaim,
    world_id: &str,
) -> Result<(), ApplicationError> {
    let valid = sqlx::query(
        "SELECT mailbox_key FROM public.mail_poll_state
         WHERE mailbox_key = $1 AND claim_token = $2 AND claim_owner = $3
           AND state_version = $4 AND claim_expires_at > clock_timestamp()
         FOR UPDATE",
    )
    .bind(&claim.mailbox_key)
    .bind(claim.claim_token)
    .bind(&claim.worker_id)
    .bind(claim.state_version)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|_| storage_error("读取 MoogleMail 待读租约失败"))?;
    if valid.is_none() {
        return Err(storage_error("MoogleMail 待读租约已失效"));
    }
    // SELECT 的谓词可能在等待行锁之前求值；取得锁后重新核对数据库实时有效期。
    check_live_claim(transaction, claim).await?;
    let other_world: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM public.mail_poll_pending_reads
                        WHERE mailbox_key = $1 AND world_id <> $2)",
    )
    .bind(&claim.mailbox_key)
    .bind(world_id)
    .fetch_one(&mut **transaction)
    .await
    .map_err(|_| storage_error("核验 MoogleMail 待读世界失败"))?;
    if other_world {
        return Err(storage_error("MoogleMail 待读邮箱世界冲突"));
    }
    Ok(())
}

async fn check_live_claim(
    transaction: &mut Transaction<'_, Postgres>,
    claim: &MailPollClaim,
) -> Result<(), ApplicationError> {
    let valid: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM public.mail_poll_state
                        WHERE mailbox_key = $1 AND claim_token = $2 AND claim_owner = $3
                          AND state_version = $4 AND claim_expires_at > clock_timestamp())",
    )
    .bind(&claim.mailbox_key)
    .bind(claim.claim_token)
    .bind(&claim.worker_id)
    .bind(claim.state_version)
    .fetch_one(&mut **transaction)
    .await
    .map_err(|_| storage_error("核验 MoogleMail 待读租约失败"))?;
    if !valid {
        return Err(storage_error("MoogleMail 待读租约已失效"));
    }
    Ok(())
}

pub(super) async fn complete_prefix(
    transaction: &mut Transaction<'_, Postgres>,
    mailbox_key: &str,
    completed_cursor: Option<&str>,
) -> Result<(), ApplicationError> {
    // 队列只按最早前缀读取；cursor 来自最后完成项，不能删除未完成后缀。
    sqlx::query(
        "DELETE FROM public.mail_poll_pending_reads
         WHERE mailbox_key = $1 AND read_sequence <= (
             SELECT read_sequence FROM public.mail_poll_pending_reads
             WHERE mailbox_key = $1 AND external_mail_id = $2)",
    )
    .bind(mailbox_key)
    .bind(completed_cursor)
    .execute(&mut **transaction)
    .await
    .map_err(|_| storage_error("完成 MoogleMail 待读前缀失败"))?;
    Ok(())
}
