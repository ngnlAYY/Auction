//! sending 线性化与完成；取消不覆盖已发送/未知结果，所有更新带租约与 fence。
use super::*;

pub(super) async fn start(
    store: &DatabaseStore,
    claim: LotWatchPmClaim,
    sender: String,
) -> Result<bool, LotWatchError> {
    if claim.reconcile || claim.sender.is_some() || !valid_forum_pm_id(&sender) {
        return Err(LotWatchError::Invalid);
    }
    let DatabaseStore::Postgres(store) = store;
    let mut tx = store.pool().begin().await.map_err(sql_error)?;
    // 必须先 lot 后任务；与取消/重新领先同一个线性化锁。
    let locked: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM public.lots WHERE id=$1 FOR UPDATE")
            .bind(claim.lot_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(sql_error)?;
    if locked.is_none() {
        return Err(LotWatchError::Unavailable);
    }
    let row = sqlx::query("SELECT q.notification_id,n.command_id,n.payload,n.request_digest,n.recipient,
        (n.lot_id=$4 AND n.channel='pm' AND n.delivery_id IS NULL AND n.preference_version > 0
        AND n.invalidated_at IS NULL AND w.watched AND w.channel='mm' AND w.version >= n.watch_version
        AND l.leader_user_id IS DISTINCT FROM n.user_id
        AND lot.effective_end_at IS NOT NULL AND lot.effective_end_at > clock_timestamp()) AS eligible
        FROM public.lot_watch_pm_deliveries q JOIN public.lot_watch_notifications n ON n.id=q.notification_id
        JOIN public.lot_watches w ON w.user_id=n.user_id AND w.lot_id=n.lot_id
        JOIN public.lot_watch_leadership l ON l.lot_id=n.lot_id
        JOIN public.lots lot ON lot.id=n.lot_id
        WHERE q.notification_id=$1 AND q.lease_token=$2 AND q.fencing_version=$3
        AND q.status='claimed' AND q.sending_at IS NULL AND q.lease_kind='send'
        AND q.lease_expires_at > clock_timestamp() FOR UPDATE OF q")
        .bind(claim.message.notification_id).bind(claim.token).bind(claim.fence).bind(claim.lot_id)
        .fetch_optional(&mut *tx).await.map_err(sql_error)?;
    let Some(row) = row else {
        return Err(LotWatchError::Conflict);
    };
    let frozen = message(&row)?;
    let digest: Vec<u8> = row.try_get("request_digest").map_err(sql_error)?;
    if frozen != claim.message || digest != claim.digest {
        return Err(LotWatchError::Unavailable);
    }
    let eligible: bool = row.try_get("eligible").map_err(sql_error)?;
    if !eligible {
        let count = sqlx::query("UPDATE public.lot_watch_pm_deliveries SET status='suppressed',failure_code='source_invalid',
            lease_token=NULL,lease_expires_at=NULL,lease_kind=NULL,worker_id=NULL,completed_at=clock_timestamp(),updated_at=clock_timestamp()
            WHERE notification_id=$1 AND lease_token=$2 AND fencing_version=$3 AND sending_at IS NULL
            AND lease_expires_at > clock_timestamp()")
            .bind(claim.message.notification_id).bind(claim.token).bind(claim.fence)
            .execute(&mut *tx).await.map_err(sql_error)?.rows_affected();
        if count != 1 {
            return Err(LotWatchError::Conflict);
        }
        tx.commit().await.map_err(sql_error)?;
        return Ok(false);
    }
    let count = sqlx::query(
        "UPDATE public.lot_watch_pm_deliveries SET status='sending',sending_at=clock_timestamp(),
        sender_forum_account_id=$4,failure_code=NULL,updated_at=clock_timestamp()
        WHERE notification_id=$1 AND lease_token=$2 AND fencing_version=$3 AND status='claimed'
        AND sending_at IS NULL AND lease_kind='send' AND lease_expires_at > clock_timestamp()",
    )
    .bind(claim.message.notification_id)
    .bind(claim.token)
    .bind(claim.fence)
    .bind(sender)
    .execute(&mut *tx)
    .await
    .map_err(sql_error)?
    .rows_affected();
    if count != 1 {
        return Err(LotWatchError::Conflict);
    }
    // 提交响应未知时不授权调用 send，留给租约恢复及只读对账。
    tx.commit().await.map_err(sql_error)?;
    Ok(true)
}

pub(super) async fn release(
    store: &DatabaseStore,
    claim: LotWatchPmClaim,
) -> Result<(), LotWatchError> {
    if claim.reconcile || claim.sender.is_some() {
        return Err(LotWatchError::Invalid);
    }
    let DatabaseStore::Postgres(store) = store;
    let mut tx = store.pool().begin().await.map_err(sql_error)?;
    lock_delivery(&mut tx, claim.message.notification_id).await?;
    let count = sqlx::query("UPDATE public.lot_watch_pm_deliveries SET status='pending',failure_code='prepare_unavailable',
        lease_token=NULL,lease_expires_at=NULL,lease_kind=NULL,worker_id=NULL,updated_at=clock_timestamp()
        WHERE notification_id=$1 AND lease_token=$2 AND fencing_version=$3 AND status='claimed'
        AND sending_at IS NULL AND lease_kind='send' AND lease_expires_at > clock_timestamp()")
        .bind(claim.message.notification_id).bind(claim.token).bind(claim.fence).execute(&mut *tx).await.map_err(sql_error)?.rows_affected();
    if count != 1 {
        return Err(LotWatchError::Conflict);
    }
    tx.commit().await.map_err(sql_error)?;
    Ok(())
}

pub(super) async fn finish(
    store: &DatabaseStore,
    claim: LotWatchPmClaim,
    receipt: Option<ForumPmReceipt>,
) -> Result<(), LotWatchError> {
    let sender =
        claim.sender.as_deref().filter(|id| valid_forum_pm_id(id)).ok_or(LotWatchError::Invalid)?;
    let external = match receipt {
        Some(receipt)
            if receipt.message == claim.message
                && receipt.sender_forum_account_id == sender
                && valid_forum_pm_id(&receipt.sent_message_id) =>
        {
            Some(receipt.sent_message_id)
        }
        Some(_) => return Err(LotWatchError::Unavailable),
        None => None,
    };
    if forum_pm_digest(&claim.message)?.as_bytes().as_slice() != claim.digest.as_slice() {
        return Err(LotWatchError::Unavailable);
    }
    let DatabaseStore::Postgres(store) = store;
    let mut tx = store.pool().begin().await.map_err(sql_error)?;
    lock_delivery(&mut tx, claim.message.notification_id).await?;
    // 完成不再检查 watch，发送后的取消不能擦除已发生的远端结果。
    let count = sqlx::query("UPDATE public.lot_watch_pm_deliveries q SET status=CASE WHEN $5::text IS NULL THEN 'manual_reconcile' ELSE 'succeeded' END,
        external_reference=$5,failure_code=CASE WHEN $5::text IS NOT NULL THEN NULL WHEN $6 THEN 'evidence_missing' ELSE 'outcome_unknown' END,
        completed_at=CASE WHEN $5::text IS NULL THEN NULL ELSE clock_timestamp() END,
        reconcile_requested_at=CASE WHEN $5::text IS NULL AND NOT $6 THEN clock_timestamp() ELSE NULL END,
        lease_token=NULL,lease_expires_at=NULL,lease_kind=NULL,worker_id=NULL,updated_at=clock_timestamp()
        FROM public.lot_watch_notifications n WHERE q.notification_id=$1 AND q.lease_token=$2 AND q.fencing_version=$3
        AND q.sender_forum_account_id=$4 AND q.sending_at IS NOT NULL
        AND q.lease_expires_at > clock_timestamp()
        AND (($6 AND q.status='manual_reconcile' AND q.lease_kind='reconcile') OR (NOT $6 AND q.status='sending' AND q.lease_kind='send'))
        AND n.id=q.notification_id AND n.channel='pm' AND n.request_digest=$7 AND n.payload=$8 AND n.recipient=$9")
        .bind(claim.message.notification_id).bind(claim.token).bind(claim.fence).bind(sender).bind(external).bind(claim.reconcile)
        .bind(&claim.digest).bind(serde_json::to_value(&claim.message).map_err(|_| LotWatchError::Unavailable)?).bind(&claim.message.recipient_forum_account_id)
        .execute(&mut *tx).await.map_err(sql_error)?.rows_affected();
    if count != 1 {
        return Err(LotWatchError::Conflict);
    }
    tx.commit().await.map_err(sql_error)?;
    Ok(())
}

/// 释放与完成只锁队列；取得锁后重新计算 TTL，不反向取得 lot 锁。
async fn lock_delivery(
    tx: &mut Transaction<'_, Postgres>,
    notification_id: Uuid,
) -> Result<(), LotWatchError> {
    sqlx::query(
        "SELECT notification_id FROM public.lot_watch_pm_deliveries
         WHERE notification_id = $1 FOR UPDATE",
    )
    .bind(notification_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(sql_error)?;
    Ok(())
}
