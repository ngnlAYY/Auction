//! CLI 安全列表与只读对账请求；复用命令账本，不接触论坛凭据。
use super::*;
use auction_application::{
    LotWatchPmOperations, LotWatchPmSnapshot, RequestHash, RequestLotWatchPmReconcile,
    StoredCommand, TransactionPolicy, TransactionRunner,
};

const KIND: &str = "request_lot_watch_pm_reconcile";

impl LotWatchPmOperations for DatabaseStore {
    fn list(
        &self,
        status: Option<String>,
        limit: i64,
        offset: i64,
    ) -> LotWatchFuture<'_, Vec<LotWatchPmSnapshot>> {
        Box::pin(async move {
            if !(1..=100).contains(&limit)
                || !(0..=10000).contains(&offset)
                || status.as_deref().is_some_and(|status| {
                    !matches!(
                        status,
                        "pending"
                            | "claimed"
                            | "sending"
                            | "succeeded"
                            | "suppressed"
                            | "manual_reconcile"
                    )
                })
            {
                return Err(LotWatchError::Invalid);
            }
            let Self::Postgres(store) = self;
            let rows = sqlx::query("SELECT q.notification_id,n.lot_id,q.status,q.fencing_version,q.sender_forum_account_id,q.failure_code,q.sending_at,q.completed_at,q.reconcile_requested_at FROM public.lot_watch_pm_deliveries q JOIN public.lot_watch_notifications n ON n.id=q.notification_id WHERE ($1::text IS NULL OR q.status=$1) ORDER BY q.created_at,q.notification_id LIMIT $2 OFFSET $3")
                .bind(status).bind(limit).bind(offset).fetch_all(store.pool()).await.map_err(sql_error)?;
            rows.iter().map(snapshot).collect()
        })
    }
    fn request_reconcile(
        &self,
        command: RequestLotWatchPmReconcile,
    ) -> LotWatchFuture<'_, LotWatchPmSnapshot> {
        Box::pin(async move {
            if command.command_id.is_nil()
                || command.notification_id.is_nil()
                || command.expected_fencing_version <= 0
                || command.actor.is_empty()
                || command.actor.len() > 128
                || command.actor.trim() != command.actor
                || command.actor.chars().any(char::is_control)
            {
                return Err(LotWatchError::Invalid);
            }
            let hash = RequestHash::from_request(KIND, command.notification_id, &command)
                .map_err(|_| LotWatchError::Invalid)?;
            let Self::Postgres(store) = self;
            store.for_transaction(auction_observability::DatabaseTransactionOwner::PublicWatch)
    .run(TransactionPolicy::serializable(), |transaction| {
                let command = command.clone();
                Box::pin(async move {
                    // 对账只锁队列，不再倒取 lot；此命令没有发送资格。
                    let row = sqlx::query("SELECT q.notification_id,n.lot_id,q.status,q.fencing_version,q.sender_forum_account_id,q.failure_code,q.sending_at,q.completed_at,q.reconcile_requested_at FROM public.lot_watch_pm_deliveries q JOIN public.lot_watch_notifications n ON n.id=q.notification_id WHERE q.notification_id=$1 FOR UPDATE OF q")
                        .bind(command.notification_id).fetch_optional(&mut **transaction.transaction_mut()?).await.map_err(sql_error)?.ok_or(LotWatchError::NotFound)?;
                    if let Some(stored) = transaction.find_command_inner(command.command_id).await? {
                        if !stored.matches_request(KIND,command.notification_id,hash) { return Err(LotWatchError::Conflict); }
                        let result: LotWatchPmSnapshot = serde_json::from_slice(&stored.result_payload).map_err(|_| LotWatchError::Unavailable)?;
                        if stored.result_code != "ok" || result.notification_id != command.notification_id || Some(result.fencing_version) != command.expected_fencing_version.checked_add(1) || result.status != "manual_reconcile" || result.reconcile_requested_at.is_none() { return Err(LotWatchError::Unavailable); }
                        return Ok(result);
                    }
                    let current = snapshot(&row)?;
                    if current.fencing_version != command.expected_fencing_version || current.status != "manual_reconcile" { return Err(LotWatchError::Conflict); }
                    let next = current.fencing_version.checked_add(1).ok_or(LotWatchError::Unavailable)?;
                    let tx = transaction.transaction_mut()?;
                    let changed = sqlx::query("UPDATE public.lot_watch_pm_deliveries SET fencing_version=$2,reconcile_requested_at=clock_timestamp(),
                        lease_token=NULL,lease_expires_at=NULL,lease_kind=NULL,worker_id=NULL,updated_at=clock_timestamp()
                        WHERE notification_id=$1 AND status='manual_reconcile' AND sending_at IS NOT NULL
                        AND (lease_expires_at IS NULL OR lease_expires_at <= clock_timestamp())")
                        .bind(command.notification_id).bind(next).execute(&mut **tx).await.map_err(sql_error)?.rows_affected();
                    if changed != 1 { return Err(LotWatchError::Conflict); }
                    audit_request(tx,&command,next).await?;
                    let row = sqlx::query("SELECT q.notification_id,n.lot_id,q.status,q.fencing_version,q.sender_forum_account_id,q.failure_code,q.sending_at,q.completed_at,q.reconcile_requested_at FROM public.lot_watch_pm_deliveries q JOIN public.lot_watch_notifications n ON n.id=q.notification_id WHERE q.notification_id=$1")
                        .bind(command.notification_id).fetch_one(&mut **tx).await.map_err(sql_error)?;
                    let result = snapshot(&row)?;
                    transaction.insert_command_inner(&StoredCommand { command_id:command.command_id,command_kind:KIND.to_owned(),aggregate_id:command.notification_id,request_hash:hash,result_code:"ok".to_owned(),result_payload:serde_json::to_vec(&result).map_err(|_| LotWatchError::Unavailable)? }).await.map_err(|error| match error {
                        auction_application::CompetitionTransactionError::CommandLedger(auction_application::CommandLedgerError::Conflict) => LotWatchError::Conflict,
                        error => error.into(),
                    })?;
                    Ok(result)
                })
            }).await
        })
    }
}

fn snapshot(row: &sqlx::postgres::PgRow) -> Result<LotWatchPmSnapshot, LotWatchError> {
    Ok(LotWatchPmSnapshot {
        notification_id: row.try_get("notification_id").map_err(sql_error)?,
        lot_id: row.try_get("lot_id").map_err(sql_error)?,
        status: row.try_get("status").map_err(sql_error)?,
        fencing_version: row.try_get("fencing_version").map_err(sql_error)?,
        sender_forum_account_id: row.try_get("sender_forum_account_id").map_err(sql_error)?,
        failure_code: row.try_get("failure_code").map_err(sql_error)?,
        sending_at: row.try_get("sending_at").map_err(sql_error)?,
        completed_at: row.try_get("completed_at").map_err(sql_error)?,
        reconcile_requested_at: row.try_get("reconcile_requested_at").map_err(sql_error)?,
    })
}
async fn audit_request(
    tx: &mut Transaction<'_, Postgres>,
    command: &RequestLotWatchPmReconcile,
    next: i64,
) -> Result<(), LotWatchError> {
    use crate::database::audit_health::AuditWriteResult as _;
    sqlx::query("INSERT INTO public.audit_records (id,actor_type,actor_id,action,entity_type,entity_id,result,source,metadata)
        VALUES ($1,'admin',$2,'lot_watch_pm.reconcile_requested','lot_watch_notification',$3,'success','system',$4)")
        .bind(Uuid::new_v4()).bind(&command.actor).bind(command.notification_id.to_string()).bind(serde_json::json!({"command_id":command.command_id,"fencing_version":next}))
        .execute(&mut **tx).await.record_audit_failure().map_err(sql_error)?;
    Ok(())
}
