//! 外部账号与通知面的水位快照。

use super::UNDISPOSED;
use crate::database::storage_error_from;
use auction_application::core::ApplicationError;
use auction_application::operations::{
    OperationsAlertCredentialSnapshot, OperationsAlertForumSnapshot, OperationsAlertMailSnapshot,
    OperationsAlertNotificationSnapshot,
};
use chrono::{DateTime, Utc};
use sqlx::Row;

pub(super) async fn mail_snapshot(
    pool: &crate::database::TrackedPool<sqlx::Postgres>,
    now: DateTime<Utc>,
) -> Result<OperationsAlertMailSnapshot, ApplicationError> {
    // 显式暂停的邮箱不算停滞，Admin 的暂停是决定而不是故障。
    // 没有任何轮询行表示尚未开始轮询（首次轮询会建立行），此时不能凭空判停。
    let row = sqlx::query(
        "SELECT
            COALESCE(GREATEST(0, EXTRACT(EPOCH FROM ($1::timestamptz - MIN(last_poll_at)))::bigint), 0)
                AS last_poll_age,
            COALESCE(MAX(consecutive_failures), 0)::bigint AS consecutive_failures,
            (SELECT MAX(checkpoint.last_full_pages_scanned)::bigint
             FROM public.mail_scan_checkpoints AS checkpoint
             JOIN (
                 SELECT id, state_version
                 FROM public.credential_keys
                 WHERE credential_kind = 'mail_cookie' AND status = 'active'
                 ORDER BY created_at DESC, id DESC
                 LIMIT 1
             ) AS current_credential
               ON current_credential.id = checkpoint.credential_id
              AND current_credential.state_version = checkpoint.credential_version
             WHERE checkpoint.stream = 'sent'
               AND checkpoint.last_full_pages_observed_at >= $1 - interval '7 days')
                AS sent_pages_scanned,
            (SELECT MAX(checkpoint.last_full_page_limit)::bigint
             FROM public.mail_scan_checkpoints AS checkpoint
             JOIN (
                 SELECT id, state_version
                 FROM public.credential_keys
                 WHERE credential_kind = 'mail_cookie' AND status = 'active'
                 ORDER BY created_at DESC, id DESC
                 LIMIT 1
             ) AS current_credential
               ON current_credential.id = checkpoint.credential_id
              AND current_credential.state_version = checkpoint.credential_version
             WHERE checkpoint.stream = 'sent'
               AND checkpoint.last_full_pages_observed_at >= $1 - interval '7 days')
                AS sent_page_limit
         FROM public.mail_poll_state
         WHERE NOT paused",
    )
    .bind(now)
    .fetch_one(pool)
    .await
    .map_err(|error| storage_error_from(error, "读取 operations alert 收件轮询水位失败"))?;
    Ok(OperationsAlertMailSnapshot {
        last_poll_age_seconds: row
            .try_get("last_poll_age")
            .map_err(|error| storage_error_from(error, "读取收件轮询时间失败"))?,
        consecutive_failures: row
            .try_get("consecutive_failures")
            .map_err(|error| storage_error_from(error, "读取收件轮询失败次数失败"))?,
        sent_pages_scanned: row
            .try_get("sent_pages_scanned")
            .map_err(|error| storage_error_from(error, "读取 Sent 完整枚举页数失败"))?,
        sent_page_limit: row
            .try_get("sent_page_limit")
            .map_err(|error| storage_error_from(error, "读取 Sent 完整枚举页数上限失败"))?,
    })
}

pub(super) async fn credential_snapshot(
    pool: &crate::database::TrackedPool<sqlx::Postgres>,
) -> Result<OperationsAlertCredentialSnapshot, ApplicationError> {
    // 只看每个（种类, scope）最新的凭据行：历史失效行不构成持续告警。
    let row = sqlx::query(
        "SELECT
            COUNT(*) FILTER (WHERE credential_kind = 'mail_cookie')::bigint AS invalid_mail_cookie,
            COUNT(*) FILTER (WHERE credential_kind = 'external_api')::bigint AS invalid_external_api
         FROM (
            SELECT DISTINCT ON (credential_kind, scope) credential_kind, status
            FROM public.credential_keys
            ORDER BY credential_kind, scope, created_at DESC, id DESC
         ) AS current
         WHERE status = 'invalid'",
    )
    .fetch_one(pool)
    .await
    .map_err(|error| storage_error_from(error, "读取 operations alert 凭据状态失败"))?;
    Ok(OperationsAlertCredentialSnapshot {
        invalid_mail_cookie: row
            .try_get("invalid_mail_cookie")
            .map_err(|error| storage_error_from(error, "读取邮件凭据失效计数失败"))?,
        invalid_external_api: row
            .try_get("invalid_external_api")
            .map_err(|error| storage_error_from(error, "读取外部凭据失效计数失败"))?,
    })
}

pub(super) async fn forum_snapshot(
    pool: &crate::database::TrackedPool<sqlx::Postgres>,
    now: DateTime<Utc>,
) -> Result<OperationsAlertForumSnapshot, ApplicationError> {
    // 主题发布失败是终态；私信只有进入 manual_reconcile 才算失败，未开始发送的退役不算。
    let row = sqlx::query(
        "SELECT
            (SELECT COUNT(*) FROM public.auction_forum_sync WHERE status = 'failed')::bigint
                AS sync_failed,
            COALESCE((SELECT EXTRACT(EPOCH FROM ($1::timestamptz - MIN(updated_at)))::bigint
                FROM public.auction_forum_sync WHERE status = 'failed'), 0) AS oldest_sync_age,
            (SELECT COUNT(*) FROM public.lot_watch_pm_deliveries
                WHERE status = 'manual_reconcile')::bigint AS message_failed,
            COALESCE((SELECT EXTRACT(EPOCH FROM ($1::timestamptz - MIN(updated_at)))::bigint
                FROM public.lot_watch_pm_deliveries
                WHERE status = 'manual_reconcile'), 0) AS oldest_message_age,
            (SELECT sync.remote_page_count::bigint
             FROM public.auction_forum_sync AS sync
             JOIN public.auctions AS auction ON auction.id = sync.auction_id
             WHERE sync.status = 'published'
               AND sync.remote_page_count IS NOT NULL
               AND sync.remote_page_count_observed_at >= $1 - interval '7 days'
               AND (auction.status IN ('preview', 'live', 'final', 'settling')
                    OR (auction.status IN ('settled', 'cancelled')
                        AND sync.terminal_refresh_enabled
                        AND sync.terminal_synced_version IS NULL))
             ORDER BY sync.remote_page_count DESC, sync.auction_id
             LIMIT 1) AS remote_page_count,
            (SELECT sync.remote_page_limit::bigint
             FROM public.auction_forum_sync AS sync
             JOIN public.auctions AS auction ON auction.id = sync.auction_id
             WHERE sync.status = 'published'
               AND sync.remote_page_count IS NOT NULL
               AND sync.remote_page_count_observed_at >= $1 - interval '7 days'
               AND (auction.status IN ('preview', 'live', 'final', 'settling')
                    OR (auction.status IN ('settled', 'cancelled')
                        AND sync.terminal_refresh_enabled
                        AND sync.terminal_synced_version IS NULL))
             ORDER BY sync.remote_page_count DESC, sync.auction_id
             LIMIT 1) AS remote_page_limit",
    )
    .bind(now)
    .fetch_one(pool)
    .await
    .map_err(|error| storage_error_from(error, "读取 operations alert 论坛失败水位失败"))?;
    Ok(OperationsAlertForumSnapshot {
        sync_failed_count: row
            .try_get("sync_failed")
            .map_err(|error| storage_error_from(error, "读取论坛发布失败计数失败"))?,
        oldest_sync_failed_age_seconds: row
            .try_get("oldest_sync_age")
            .map_err(|error| storage_error_from(error, "读取论坛发布失败时间失败"))?,
        message_failed_count: row
            .try_get("message_failed")
            .map_err(|error| storage_error_from(error, "读取论坛私信失败计数失败"))?,
        oldest_message_failed_age_seconds: row
            .try_get("oldest_message_age")
            .map_err(|error| storage_error_from(error, "读取论坛私信失败时间失败"))?,
        remote_page_count: row
            .try_get("remote_page_count")
            .map_err(|error| storage_error_from(error, "读取论坛远端分页页数失败"))?,
        remote_page_limit: row
            .try_get("remote_page_limit")
            .map_err(|error| storage_error_from(error, "读取论坛远端分页页数上限失败"))?,
    })
}

pub(super) async fn notification_snapshot(
    pool: &crate::database::TrackedPool<sqlx::Postgres>,
    now: DateTime<Utc>,
) -> Result<OperationsAlertNotificationSnapshot, ApplicationError> {
    // 结算与运维告警自身不属于“非结算通知”；已处置终态与 delivery 来源共用同一谓词。
    static QUERY: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
        let mut query = String::from(
            "SELECT
                COUNT(*) FILTER (WHERE d.status = 'failed')::bigint AS failed,
                COALESCE(EXTRACT(EPOCH FROM
                    ($1::timestamptz - MIN(d.created_at) FILTER (WHERE d.status = 'failed'))
                )::bigint, 0) AS oldest_failed_age,
                COUNT(*) FILTER (WHERE d.status = 'failed' AND /* postage */)::bigint AS postage,
                COALESCE(EXTRACT(EPOCH FROM
                    ($1::timestamptz - MIN(d.created_at) FILTER (WHERE d.status = 'failed' AND /* postage */))
                )::bigint, 0) AS oldest_postage_age
             FROM public.deliveries d
             WHERE d.origin_kind NOT IN
                 ('settlement', 'settlement_recovery_action', 'operations_alert')
               AND /* undisposed */",
        );
        query = query.replace("/* undisposed */", UNDISPOSED).replace(
            "/* postage */",
            "d.failure_code IN ('send_rejected', 'send_rejected_after_stage')
                    AND d.result_metadata->>'reason_code' = 'insufficient_postage'",
        );
        query
    });
    let row = sqlx::query(sqlx::AssertSqlSafe(QUERY.as_str()))
        .bind(now)
        .fetch_one(pool)
        .await
        .map_err(|error| storage_error_from(error, "读取 operations alert 通知失败水位失败"))?;
    Ok(OperationsAlertNotificationSnapshot {
        failed_count: row
            .try_get("failed")
            .map_err(|error| storage_error_from(error, "读取通知失败计数失败"))?,
        oldest_failed_age_seconds: row
            .try_get("oldest_failed_age")
            .map_err(|error| storage_error_from(error, "读取通知失败时间失败"))?,
        postage_insufficient_count: row
            .try_get("postage")
            .map_err(|error| storage_error_from(error, "读取邮资不足计数失败"))?,
        oldest_postage_age_seconds: row
            .try_get("oldest_postage_age")
            .map_err(|error| storage_error_from(error, "读取邮资不足时间失败"))?,
    })
}
