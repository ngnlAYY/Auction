//! 论坛主题同步任务的 PostgreSQL 领取、租约和 fencing。

use auction_application::{ApplicationError, ForumSyncClaim};
use auction_domain::AuctionId;
use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::super::super::PostgresStore;
use super::validation::storage_error;

pub(super) async fn claim_due_postgres(
    store: &PostgresStore,
    worker_id: &str,
    now: DateTime<Utc>,
    lease_until: DateTime<Utc>,
    startup_recovery_before: Option<DateTime<Utc>>,
    limit: u16,
) -> Result<Vec<ForumSyncClaim>, ApplicationError> {
    use sqlx::Row;

    let mut transaction =
        store.pool().begin().await.map_err(|_| storage_error("开启论坛同步领取事务失败"))?;
    // 旧版只在拍卖首次进入 live 时建立一次主题任务；不能在 Worker 重启后为历史终态拍卖补帖。
    sqlx::query(
        "INSERT INTO public.auction_forum_sync (auction_id, source_revision)
         SELECT a.id, a.state_version
         FROM public.auctions AS a
         WHERE a.status = 'live'
         ON CONFLICT (auction_id) DO NOTHING",
    )
    .execute(&mut *transaction)
    .await
    .map_err(|_| storage_error("初始化论坛同步投影失败"))?;

    // 启动水位仅重新领取历史凭据失败或未知写入；原失败码随 claim 传给 Worker，后者只能只读对账。
    // updated_at 越过首轮水位后不再走此分支，避免同一进程热循环；人工授权门禁不在白名单内。
    let rows = sqlx::query(
        "SELECT sync.auction_id, auction.state_version, sync.thread_id AS existing_thread_id,
                CASE WHEN sync.status = 'publishing' THEN 'write_unknown'
                     ELSE sync.last_error_code END AS previous_failure_code
         FROM public.auction_forum_sync AS sync
         JOIN public.auctions AS auction ON auction.id = sync.auction_id
         WHERE auction.opened_at IS NOT NULL
           AND (
               (sync.status IN ('pending', 'failed')
                AND (sync.last_error_code IS NULL OR sync.last_error_code NOT IN (
                    'invalid_snapshot', 'body_too_large', 'configuration',
                    'credential_unavailable', 'protocol_drift', 'fixed_layout_conflict',
                    'write_authorization_required'
                ))
                AND sync.next_attempt_at <= $1)
               OR ($3::timestamptz IS NOT NULL
                   AND sync.status = 'failed'
                   AND sync.last_error_code IN (
                       'credential_unavailable', 'write_unknown', 'publish_proof_failed',
                       'reconcile_write_unknown', 'reconcile_publish_proof_failed'
                   )
                   AND sync.updated_at < $3)
               OR (sync.status = 'publishing'
                   AND (sync.lease_expires_at IS NULL OR sync.lease_expires_at <= $1))
               OR (sync.status = 'published'
                   AND (sync.catalog_post_id IS NULL OR sync.system_log_post_id IS NULL))
           )
         ORDER BY sync.next_attempt_at, sync.auction_id
         LIMIT $2
         FOR UPDATE OF sync SKIP LOCKED",
    )
    .bind(now)
    .bind(i64::from(limit))
    .bind(startup_recovery_before)
    .fetch_all(&mut *transaction)
    .await
    .map_err(|_| storage_error("读取 due 论坛同步任务失败"))?;

    let mut claims = Vec::with_capacity(rows.len());
    for row in rows {
        let auction_id: Uuid =
            row.try_get("auction_id").map_err(|_| storage_error("读取论坛同步拍卖标识失败"))?;
        let source_revision: i64 =
            row.try_get("state_version").map_err(|_| storage_error("读取论坛同步源版本失败"))?;
        let previous_failure_code: Option<String> = row
            .try_get("previous_failure_code")
            .map_err(|_| storage_error("读取论坛同步前序失败码失败"))?;
        let existing_thread_id: Option<String> = row
            .try_get("existing_thread_id")
            .map_err(|_| storage_error("读取论坛同步历史主题标识失败"))?;
        if source_revision <= 0 {
            return Err(storage_error("论坛同步源版本无效"));
        }
        let claim_token = Uuid::new_v4();
        let updated = sqlx::query(
            "UPDATE public.auction_forum_sync
             SET status = 'publishing',
                 claim_token = $2,
                 claim_owner = $3,
                 claim_fencing = claim_fencing + 1,
                 lease_expires_at = $4,
                 source_revision = $5,
                 attempt_count = attempt_count + 1,
                 last_error_code = NULL,
                 last_error_detail = NULL,
                 updated_at = $6
             WHERE auction_id = $1
               AND (claim_token IS NULL OR lease_expires_at IS NULL OR lease_expires_at <= $6)
             RETURNING claim_fencing",
        )
        .bind(auction_id)
        .bind(claim_token)
        .bind(worker_id)
        .bind(lease_until)
        .bind(source_revision)
        .bind(now)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|_| storage_error("写入论坛同步租约失败"))?;
        let Some(updated) = updated else {
            return Err(storage_error("论坛同步租约在事务内发生变化"));
        };
        let claim_fencing: i64 = updated
            .try_get("claim_fencing")
            .map_err(|_| storage_error("读取论坛同步 fencing 失败"))?;
        if claim_fencing < 1 {
            return Err(storage_error("论坛同步 fencing 无效"));
        }
        claims.push(ForumSyncClaim {
            auction_id: AuctionId::from_uuid(auction_id),
            claim_token,
            claim_fencing,
            worker_id: worker_id.to_owned(),
            claimed_at: now,
            lease_until,
            source_revision,
            existing_thread_id,
            previous_failure_code,
        });
    }
    transaction.commit().await.map_err(|_| storage_error("提交论坛同步领取事务失败"))?;
    Ok(claims)
}
