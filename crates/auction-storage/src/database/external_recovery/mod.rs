//! W10.9 恢复后外部核对的本地读取与隔离写入。
//!
//! 读取只返回冻结的发送预期、已知入站邮件标识与论坛游标，不展开正文、Cookie 或凭据。
//! 隔离在单事务内重新锁定投递、核对 fence 与当前发送预期，然后把 pending/claimed 改为
//! `manual_reconcile` 并追加隔离事实与审计。绝不重发、领取或改写资金/结算事实。

use std::collections::BTreeSet;

use auction_application::core::ApplicationError;
use auction_application::delivery::DeliverySendExpectation;
use auction_application::recovery::{
    EXTERNAL_RECOVERY_RECEIPT_FORMAT, ExternalRecoveryDeliveryExpectation,
    ExternalRecoveryForumThread, ExternalRecoveryQuarantineMatch,
    ExternalRecoveryQuarantineOutcome, ExternalRecoveryStore, MAX_EXTERNAL_RECOVERY_DELIVERIES,
};
use chrono::{DateTime, Utc};
use sqlx::{Postgres, Row};
use uuid::Uuid;

use super::DatabaseStore;
use crate::database::{
    TrackedPool, delivery_audit, delivery_audit::DeliveryAudit, storage_error, storage_error_from,
};

/// 恢复核对隔离的审计动作；只表示本地状态收口，不证明发送、领取或收款。
pub const EXTERNAL_RECOVERY_QUARANTINE_ACTION: &str = "delivery.external_recovery_quarantine";
/// 投递隔离后的失败码；Admin 仍可按既有 manual_reconcile 动作人工处置。
pub const EXTERNAL_RECOVERY_QUARANTINE_FAILURE_CODE: &str = "external_recovery_sent_match";

impl ExternalRecoveryStore for DatabaseStore {
    fn pending_claimed_send_expectations(
        &self,
    ) -> auction_application::recovery::ExternalRecoveryFuture<
        '_,
        Vec<ExternalRecoveryDeliveryExpectation>,
    > {
        Box::pin(async move {
            let Self::Postgres(store) = self;
            read_expectations(store.pool()).await
        })
    }

    fn known_inbound_mail_ids(
        &self,
        candidates: Vec<(String, String)>,
    ) -> auction_application::recovery::ExternalRecoveryFuture<'_, Vec<(String, String)>> {
        Box::pin(async move {
            if candidates.len() > MAX_EXTERNAL_RECOVERY_DELIVERIES * 4 {
                return Err(storage_error("已知入站邮件核对候选过多"));
            }
            let mut worlds = Vec::with_capacity(candidates.len());
            let mut mail_ids = Vec::with_capacity(candidates.len());
            for (world, mail_id) in &candidates {
                if !matches!(world.as_str(), "main" | "isekai")
                    || !auction_domain::valid_positive_decimal(mail_id, 64)
                {
                    return Err(storage_error("已知入站邮件核对候选无效"));
                }
                worlds.push(world.clone());
                mail_ids.push(mail_id.clone());
            }
            let Self::Postgres(store) = self;
            let rows = sqlx::query(
                "WITH wanted(world_id, external_mail_id) AS (
                     SELECT * FROM unnest($1::text[], $2::text[])
                 )
                 SELECT DISTINCT wanted.world_id, wanted.external_mail_id
                 FROM wanted
                 WHERE EXISTS (
                         SELECT 1 FROM public.inbound_mail AS mail
                         WHERE mail.world_id = wanted.world_id
                           AND mail.external_mail_id = wanted.external_mail_id)
                    OR EXISTS (
                         SELECT 1 FROM public.mail_poll_pending_reads AS pending
                         WHERE pending.world_id = wanted.world_id
                           AND pending.external_mail_id = wanted.external_mail_id)
                    OR EXISTS (
                         SELECT 1
                         FROM public.auction_submissions AS submission
                         JOIN public.auctions AS auction ON auction.id = submission.auction_id
                         WHERE auction.world_id = wanted.world_id
                           AND submission.external_mail_id = wanted.external_mail_id)",
            )
            .bind(&worlds)
            .bind(&mail_ids)
            .fetch_all(store.pool())
            .await
            .map_err(|error| storage_error_from(error, "读取已知入站邮件失败"))?;
            let mut known = Vec::with_capacity(rows.len());
            for row in rows {
                let world: String = row
                    .try_get("world_id")
                    .map_err(|error| storage_error_from(error, "读取已知入站邮件世界失败"))?;
                let mail_id: String = row
                    .try_get("external_mail_id")
                    .map_err(|error| storage_error_from(error, "读取已知入站邮件标识失败"))?;
                known.push((world, mail_id));
            }
            Ok(known)
        })
    }

    fn published_forum_threads(
        &self,
        limit: u16,
    ) -> auction_application::recovery::ExternalRecoveryFuture<'_, Vec<ExternalRecoveryForumThread>>
    {
        Box::pin(async move {
            if limit == 0
                || limit > auction_application::recovery::MAX_EXTERNAL_RECOVERY_FORUM_THREADS
            {
                return Err(storage_error("论坛恢复核对主题上限无效"));
            }
            let Self::Postgres(store) = self;
            let rows = sqlx::query(
                "SELECT sync.auction_id, auction.world_id, sync.thread_id,
                        sync.processed_through_post_number
                 FROM public.auction_forum_sync AS sync
                 JOIN public.auctions AS auction ON auction.id = sync.auction_id
                 WHERE sync.status = 'published'
                   AND sync.thread_id IS NOT NULL
                 ORDER BY sync.updated_at DESC, sync.auction_id
                 LIMIT $1",
            )
            .bind(i64::from(limit))
            .fetch_all(store.pool())
            .await
            .map_err(|error| storage_error_from(error, "读取已发布论坛主题失败"))?;
            let mut threads = Vec::with_capacity(rows.len());
            for row in rows {
                let thread = ExternalRecoveryForumThread {
                    auction_id: row
                        .try_get("auction_id")
                        .map_err(|error| storage_error_from(error, "读取论坛主题拍卖标识失败"))?,
                    world_id: row
                        .try_get("world_id")
                        .map_err(|error| storage_error_from(error, "读取论坛主题世界失败"))?,
                    thread_id: row
                        .try_get("thread_id")
                        .map_err(|error| storage_error_from(error, "读取论坛主题标识失败"))?,
                    after_post_number: u32::try_from(
                        row.try_get::<i32, _>("processed_through_post_number")
                            .map_err(|error| storage_error_from(error, "读取论坛已处理楼层失败"))?,
                    )
                    .map_err(|error| {
                        let _ = error;
                        storage_error("论坛已处理楼层超出范围")
                    })?,
                };
                if !thread.validate() {
                    return Err(storage_error("已发布论坛主题投影无效"));
                }
                threads.push(thread);
            }
            Ok(threads)
        })
    }

    fn quarantine_external_sent_matches(
        &self,
        scan_id: Uuid,
        actor_id: String,
        matches: Vec<ExternalRecoveryQuarantineMatch>,
    ) -> auction_application::recovery::ExternalRecoveryFuture<
        '_,
        Vec<ExternalRecoveryQuarantineOutcome>,
    > {
        Box::pin(async move {
            if scan_id.is_nil() {
                return Err(storage_error("恢复扫描标识不能为空"));
            }
            if actor_id.is_empty()
                || actor_id.len() > 128
                || actor_id != actor_id.trim()
                || actor_id.chars().any(char::is_control)
            {
                return Err(storage_error("恢复核对操作者标识无效"));
            }
            if matches.len() > MAX_EXTERNAL_RECOVERY_DELIVERIES {
                return Err(storage_error("恢复核对隔离批次过大"));
            }
            if matches.iter().any(|m| !m.validate()) {
                return Err(storage_error("恢复核对隔离匹配无效"));
            }
            let mut seen = BTreeSet::new();
            if matches.iter().any(|m| !seen.insert((m.delivery_id, m.fencing_version))) {
                return Err(storage_error("恢复核对隔离匹配重复"));
            }
            let Self::Postgres(store) = self;
            let now = Utc::now();
            let mut transaction = store
                .pool()
                .begin_with("BEGIN ISOLATION LEVEL READ COMMITTED")
                .await
                .map_err(|error| storage_error_from(error, "开启恢复隔离事务失败"))?;
            let mut outcomes = Vec::with_capacity(matches.len());
            for candidate in matches {
                let outcome =
                    quarantine_one(&mut transaction, scan_id, &actor_id, &candidate, now).await?;
                outcomes.push(outcome);
            }
            transaction
                .commit()
                .await
                .map_err(|error| storage_error_from(error, "提交恢复隔离事务失败"))?;
            Ok(outcomes)
        })
    }
}

async fn read_expectations(
    pool: &TrackedPool<Postgres>,
) -> Result<Vec<ExternalRecoveryDeliveryExpectation>, ApplicationError> {
    let rows = sqlx::query(
        "SELECT delivery.id, delivery.fencing_version, delivery.recipient,
                attempt.world_id, attempt.sending_at, attempt.expectation
         FROM public.deliveries AS delivery
         JOIN public.delivery_send_attempts AS attempt
           ON attempt.delivery_id = delivery.id
          AND attempt.fencing_version = delivery.fencing_version
         WHERE delivery.status IN ('pending', 'claimed')
           AND delivery.payload_operation = 'send'
         ORDER BY attempt.sending_at ASC, delivery.id
         LIMIT $1",
    )
    .bind(i64::try_from(MAX_EXTERNAL_RECOVERY_DELIVERIES).unwrap_or(i64::MAX))
    .fetch_all(pool)
    .await
    .map_err(|error| storage_error_from(error, "读取待核对投递发送预期失败"))?;
    let mut expectations = Vec::with_capacity(rows.len());
    for row in rows {
        let expectation: serde_json::Value = row
            .try_get("expectation")
            .map_err(|error| storage_error_from(error, "读取投递发送预期失败"))?;
        let expectation: DeliverySendExpectation =
            serde_json::from_value(expectation).map_err(|error| {
                let _ = error;
                storage_error("投递发送预期格式无效")
            })?;
        if !expectation.validate() {
            return Err(storage_error("投递发送预期违反发送不变量"));
        }
        let delivery_id: Uuid =
            row.try_get("id").map_err(|error| storage_error_from(error, "读取投递标识失败"))?;
        let fencing_version: i64 = row
            .try_get("fencing_version")
            .map_err(|error| storage_error_from(error, "读取投递 fence 失败"))?;
        let recipient: String = row
            .try_get("recipient")
            .map_err(|error| storage_error_from(error, "读取投递收件人失败"))?;
        if !recipient.eq_ignore_ascii_case(&expectation.recipient) {
            return Err(storage_error("投递收件人与冻结发送预期不一致"));
        }
        expectations.push(ExternalRecoveryDeliveryExpectation {
            delivery_id,
            fencing_version,
            world_id: expectation.world_id,
            recipient: expectation.recipient,
            subject: expectation.subject,
            credits: expectation.credits,
            cod: expectation.cod,
            equipment_ids: expectation.equipment_ids,
            sending_at: row
                .try_get("sending_at")
                .map_err(|error| storage_error_from(error, "读取投递发送时间失败"))?,
        });
    }
    Ok(expectations)
}

async fn quarantine_one(
    transaction: &mut crate::database::TrackedTransaction<'_, Postgres>,
    scan_id: Uuid,
    actor_id: &str,
    candidate: &ExternalRecoveryQuarantineMatch,
    now: DateTime<Utc>,
) -> Result<ExternalRecoveryQuarantineOutcome, ApplicationError> {
    let row = sqlx::query(
        "SELECT status, fencing_version FROM public.deliveries WHERE id = $1 FOR UPDATE",
    )
    .bind(candidate.delivery_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|error| storage_error_from(error, "锁定恢复隔离投递失败"))?;
    let Some(row) = row else {
        return Ok(ExternalRecoveryQuarantineOutcome::skipped(
            candidate.delivery_id,
            "delivery_missing",
        ));
    };
    let status: String =
        row.try_get("status").map_err(|error| storage_error_from(error, "读取投递状态失败"))?;
    let fencing_version: i64 = row
        .try_get("fencing_version")
        .map_err(|error| storage_error_from(error, "读取投递 fence 失败"))?;
    if !matches!(status.as_str(), "pending" | "claimed") {
        return Ok(ExternalRecoveryQuarantineOutcome::skipped(
            candidate.delivery_id,
            "not_pending_or_claimed",
        ));
    }
    if fencing_version != candidate.fencing_version {
        return Ok(ExternalRecoveryQuarantineOutcome::skipped(
            candidate.delivery_id,
            "fencing_changed",
        ));
    }
    let attempt = sqlx::query(
        "SELECT expectation FROM public.delivery_send_attempts
         WHERE delivery_id = $1 AND fencing_version = $2",
    )
    .bind(candidate.delivery_id)
    .bind(candidate.fencing_version)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|error| storage_error_from(error, "读取当前发送预期失败"))?;
    let Some(attempt) = attempt else {
        return Ok(ExternalRecoveryQuarantineOutcome::skipped(
            candidate.delivery_id,
            "send_expectation_missing",
        ));
    };
    let expectation: serde_json::Value = attempt
        .try_get("expectation")
        .map_err(|error| storage_error_from(error, "读取当前发送预期失败"))?;
    let expectation: DeliverySendExpectation =
        serde_json::from_value(expectation).map_err(|error| {
            let _ = error;
            storage_error("当前发送预期格式无效")
        })?;
    if !expectation.validate()
        || expectation.world_id != candidate.world_id
        || !expectation.recipient.eq_ignore_ascii_case(&candidate.recipient)
        || expectation.subject != candidate.subject
        || expectation.credits != candidate.expected_credits
        || expectation.cod != candidate.expected_cod
        || expectation.equipment_ids != candidate.expected_equipment_ids
    {
        return Ok(ExternalRecoveryQuarantineOutcome::skipped(
            candidate.delivery_id,
            "send_expectation_changed",
        ));
    }

    let inserted = sqlx::query(
        "INSERT INTO public.delivery_external_quarantines
             (scan_id, delivery_id, fencing_version, world_id, external_mail_id, sent_at,
              recipient, subject, collected, observed_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
         ON CONFLICT (delivery_id, fencing_version) DO NOTHING
         RETURNING id",
    )
    .bind(scan_id)
    .bind(candidate.delivery_id)
    .bind(candidate.fencing_version)
    .bind(&candidate.world_id)
    .bind(&candidate.external_mail_id)
    .bind(candidate.sent_at)
    .bind(&candidate.recipient)
    .bind(&candidate.subject)
    .bind(candidate.collected)
    .bind(candidate.observed_at)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|error| storage_error_from(error, "写入投递外部隔离事实失败"))?;
    if inserted.is_none() {
        return Ok(ExternalRecoveryQuarantineOutcome::skipped(
            candidate.delivery_id,
            "already_quarantined",
        ));
    }

    let updated = sqlx::query(
        "UPDATE public.deliveries AS delivery
         SET status = 'manual_reconcile',
             lease_token = NULL,
             lease_expires_at = NULL,
             completed_at = GREATEST($3::timestamptz, delivery.created_at,
                                     COALESCE(delivery.claimed_at, delivery.created_at)),
             failure_code = $4,
             result_metadata = (
                 CASE WHEN delivery.result_metadata ? 'sweep_retries'
                      THEN jsonb_build_object('sweep_retries',
                                              delivery.result_metadata->'sweep_retries')
                      ELSE '{}'::jsonb END)
                 || jsonb_build_object(
                        'outcome_unknown', true,
                        'reason_code', $4,
                        'external_recovery_quarantine',
                        jsonb_build_object(
                            'format', $5,
                            'scan_id', $1::uuid,
                            'external_mail_id', $6,
                            'sent_at', $7,
                            'observed_at', $8)),
             fencing_version = delivery.fencing_version + 1,
             updated_at = clock_timestamp()
         WHERE delivery.id = $2
           AND delivery.status IN ('pending', 'claimed')
           AND delivery.fencing_version = $9
         RETURNING delivery.fencing_version, delivery.attempt_count",
    )
    .bind(scan_id)
    .bind(candidate.delivery_id)
    .bind(now)
    .bind(EXTERNAL_RECOVERY_QUARANTINE_FAILURE_CODE)
    .bind(EXTERNAL_RECOVERY_RECEIPT_FORMAT)
    .bind(&candidate.external_mail_id)
    .bind(candidate.sent_at)
    .bind(candidate.observed_at)
    .bind(candidate.fencing_version)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|error| storage_error_from(error, "隔离恢复投递失败"))?;
    let Some(updated) = updated else {
        // 已插入隔离事实但状态未推进；整体回滚，避免留下无状态变化的孤证。
        return Err(storage_error("隔离投递时状态在事务内发生变化"));
    };
    let next_fencing: i64 = updated
        .try_get("fencing_version")
        .map_err(|error| storage_error_from(error, "读取隔离后 fence 失败"))?;
    let attempt_count: i32 = updated
        .try_get("attempt_count")
        .map_err(|error| storage_error_from(error, "读取隔离后尝试次数失败"))?;

    delivery_audit::append(
        transaction,
        DeliveryAudit {
            occurred_at: now,
            actor_type: "cli",
            actor_id,
            action: EXTERNAL_RECOVERY_QUARANTINE_ACTION,
            delivery_id: candidate.delivery_id,
            request_id: Some(&scan_id.to_string()),
            source: "cli",
            metadata: serde_json::json!({
                "status": "manual_reconcile",
                "failure_code": EXTERNAL_RECOVERY_QUARANTINE_FAILURE_CODE,
                "fencing_version": next_fencing,
                "previous_fencing_version": candidate.fencing_version,
                "attempt_count": attempt_count,
                "external_mail_id": candidate.external_mail_id,
                "sent_at": candidate.sent_at,
                "observed_at": candidate.observed_at,
                "format": EXTERNAL_RECOVERY_RECEIPT_FORMAT,
            }),
        },
    )
    .await
    .map_err(|error| storage_error_from(error, "写入恢复隔离审计失败"))?;
    Ok(ExternalRecoveryQuarantineOutcome::quarantined(candidate.delivery_id, next_fencing))
}
