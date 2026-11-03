//! 用户偏好 SQL owner；仅锁 user，竞价读取快照不追加无序 user 锁。

use super::*;
use crate::database::{DatabaseStore, PostgresCompetitionTransaction};
use auction_application::{
    LotWatchFuture, LotWatchPreference, LotWatchPreferencePort, LotWatchPreferenceTransaction,
    SetLotWatchPreferenceCommand,
};
use sqlx::Row;

impl LotWatchPreferencePort for DatabaseStore {
    fn get(&self, user_id: Uuid) -> LotWatchFuture<'_, LotWatchPreference> {
        Box::pin(async move {
            if user_id.is_nil() {
                return Err(LotWatchError::Invalid);
            }
            let Self::Postgres(store) = self;
            let row = sqlx::query("SELECT id, lot_watch_channel, lot_watch_channel_version FROM public.users WHERE id = $1")
                .bind(user_id).fetch_optional(store.pool()).await.map_err(sql_error)?.ok_or(LotWatchError::Unavailable)?;
            map(&row)
        })
    }
    fn set(&self, command: SetLotWatchPreferenceCommand) -> LotWatchFuture<'_, LotWatchPreference> {
        Box::pin(async move {
            let Self::Postgres(store) = self;
            auction_application::set_lot_watch_preference(
                &store
                    .for_transaction(auction_observability::DatabaseTransactionOwner::PublicWatch),
                command,
            )
            .await
        })
    }
}

impl LotWatchPreferenceTransaction for PostgresCompetitionTransaction {
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
        use crate::database::audit_health::AuditWriteResult as _;
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
