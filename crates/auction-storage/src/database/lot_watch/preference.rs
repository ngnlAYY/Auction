//! 用户偏好 SQL owner；仅锁 user，竞价读取快照不追加无序 user 锁。

use crate::database::lot_watch::persistence::sql_error;
use crate::database::{DatabaseStore, PostgresCompetitionTransaction};
use auction_application::public::LotWatchError;
use auction_application::public::{
    LotWatchFuture, LotWatchPreference, LotWatchPreferencePort, LotWatchPreferenceTransaction,
    SetLotWatchPreferenceCommand,
};
use sqlx::Row;
use uuid::Uuid;

impl LotWatchPreferencePort for DatabaseStore {
    fn get(&self, user_id: Uuid) -> LotWatchFuture<'_, LotWatchPreference> {
        Box::pin(async move {
            if user_id.is_nil() {
                return Err(LotWatchError::Invalid);
            }
            let store = self;
            let row = sqlx::query("SELECT id, lot_watch_channel, lot_watch_channel_version FROM public.users WHERE id = $1")
                .bind(user_id).fetch_optional(store.pool()).await.map_err(sql_error)?.ok_or(LotWatchError::Unavailable)?;
            map(&row)
        })
    }
    fn pm_available(&self, user_id: Uuid) -> LotWatchFuture<'_, bool> {
        Box::pin(async move {
            if user_id.is_nil() {
                return Err(LotWatchError::Invalid);
            }
            let store = self;
            forum_pm_available(store.pool(), user_id).await
        })
    }
    fn set(&self, command: SetLotWatchPreferenceCommand) -> LotWatchFuture<'_, LotWatchPreference> {
        Box::pin(async move {
            let store = self;
            auction_application::public::set_lot_watch_preference(
                &store
                    .for_transaction(auction_observability::DatabaseTransactionOwner::PublicWatch),
                command,
            )
            .await
        })
    }
}

/// PM 可用性同时要求账号已绑定论坛数字 ID 与 Worker 公布的论坛同步启用状态；
/// 没有可靠启用观测时按不可用处理，避免保存一个静默失效的渠道。
async fn forum_pm_available<'e, E>(executor: E, user_id: Uuid) -> Result<bool, LotWatchError>
where
    E: sqlx::PgExecutor<'e>,
{
    sqlx::query_scalar(
        "SELECT COALESCE(
             (SELECT true FROM public.users WHERE id = $1 AND forum_account_id IS NOT NULL),
             false
         ) AND COALESCE((
             SELECT CASE
                 WHEN bool_or(details -> 'forum_sync_enabled' = 'true'::jsonb) THEN true
                 WHEN bool_and(COALESCE(details -> 'forum_sync_enabled' = 'false'::jsonb, false))
                     THEN false
                 ELSE NULL
             END
             FROM public.service_heartbeats
             WHERE service_name = 'auction-worker' AND process_role = 'worker'
               AND status IN ('starting', 'ready', 'degraded')
               AND expires_at > clock_timestamp()
         ), false)",
    )
    .bind(user_id)
    .fetch_one(executor)
    .await
    .map_err(|error| LotWatchError::Transaction(sql_error(error)))
}

impl LotWatchPreferenceTransaction for PostgresCompetitionTransaction {
    async fn forum_pm_available(&mut self, user_id: Uuid) -> Result<bool, LotWatchError> {
        forum_pm_available(&mut **self.transaction_mut()?, user_id).await
    }

    async fn lock_preference(
        &mut self,
        user_id: Uuid,
    ) -> Result<LotWatchPreference, LotWatchError> {
        let row = sqlx::query("SELECT id, lot_watch_channel, lot_watch_channel_version FROM public.users WHERE id = $1 FOR UPDATE")
            .bind(user_id).fetch_optional(&mut **self.transaction_mut()?).await.map_err(sql_error)?.ok_or(LotWatchError::Unavailable)?;
        map(&row)
    }
    async fn write_preference(
        &mut self,
        command: &SetLotWatchPreferenceCommand,
        next: i64,
    ) -> Result<(), LotWatchError> {
        use crate::database::operations::audit_health::AuditWriteResult as _;
        let tx = self.transaction_mut()?;
        let changed = sqlx::query("UPDATE public.users SET lot_watch_channel = $2, lot_watch_channel_version = $3 WHERE id = $1 AND lot_watch_channel_version = $4")
            .bind(command.user_id).bind(&command.channel).bind(next).bind(command.expected_version).execute(&mut **tx).await.map_err(sql_error)?.rows_affected();
        if changed != 1 {
            return Err(LotWatchError::Conflict);
        }
        sqlx::query("INSERT INTO public.audit_records (id, actor_type, actor_id, action, entity_type, entity_id, result, source, metadata) VALUES ($1, 'public_user', $2, 'lot_watch.preference', 'user', $2, 'success', 'public', $3)")
            .bind(Uuid::new_v4()).bind(command.user_id.to_string()).bind(serde_json::json!({"command_id":command.command_id,"channel":command.channel,"version":next}))
            .execute(&mut **tx).await.record_audit_failure().map_err(sql_error)?;
        Ok(())
    }
}

fn map(row: &sqlx::postgres::PgRow) -> Result<LotWatchPreference, LotWatchError> {
    let result = LotWatchPreference {
        user_id: row.try_get("id").map_err(sql_error)?,
        channel: row.try_get("lot_watch_channel").map_err(sql_error)?,
        version: row.try_get("lot_watch_channel_version").map_err(sql_error)?,
    };
    if result.user_id.is_nil()
        || result.version <= 0
        || !matches!(result.channel.as_str(), "mm" | "pm")
    {
        return Err(LotWatchError::Unavailable);
    }
    Ok(result)
}
