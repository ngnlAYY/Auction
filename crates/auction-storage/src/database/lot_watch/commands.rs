//! Public 标记的锁、CAS、完整读取与共用命令账本适配。

use auction_application::{
    LotWatch, LotWatchFuture, LotWatchPort, LotWatchTransaction, MAX_LOT_WATCHES,
    SetLotWatchCommand, SetLotWatchResult, StoredCommand,
};
use auction_domain::CommandId;
use sqlx::Row;

use super::*;
use crate::database::{DatabaseStore, PostgresCompetitionTransaction};

impl LotWatchPort for DatabaseStore {
    fn list(&self, user_id: Uuid, auction_id: Uuid) -> LotWatchFuture<'_, Vec<LotWatch>> {
        Box::pin(async move {
            if user_id.is_nil() || auction_id.is_nil() {
                return Err(LotWatchError::Invalid);
            }
            let Self::Postgres(store) = self;
            let mut transaction = store.pool().begin_read_snapshot().await.map_err(sql_error)?;
            let visible: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM public.auctions a
                JOIN public.worlds w ON w.id = a.world_id
                WHERE a.id = $1 AND (w.enabled OR a.status = 'settled')
                AND a.status IN ('preview', 'live', 'final', 'settling', 'settled')
                AND (a.status <> 'preview' OR NOT EXISTS (
                    SELECT 1 FROM public.auctions active WHERE active.world_id = a.world_id
                    AND active.status IN ('live', 'final', 'settling'))))",
            )
            .bind(auction_id)
            .fetch_one(&mut *transaction)
            .await
            .map_err(sql_error)?;
            if !visible {
                return Err(LotWatchError::NotFound);
            }
            let rows = sqlx::query(
                "SELECT watch.lot_id, watch.watched, watch.channel, watch.version
                FROM public.lot_watches watch JOIN public.lots lot ON lot.id = watch.lot_id
                WHERE watch.user_id = $1 AND lot.auction_id = $2 ORDER BY watch.lot_id LIMIT $3",
            )
            .bind(user_id)
            .bind(auction_id)
            .bind((MAX_LOT_WATCHES + 1) as i64)
            .fetch_all(&mut *transaction)
            .await
            .map_err(sql_error)?;
            if rows.len() > MAX_LOT_WATCHES {
                return Err(LotWatchError::TooMany);
            }
            let watches = rows.iter().map(map_watch).collect::<Result<Vec<_>, _>>()?;
            transaction.commit().await.map_err(sql_error)?;
            Ok(watches)
        })
    }

    fn set(&self, command: SetLotWatchCommand) -> LotWatchFuture<'_, SetLotWatchResult> {
        Box::pin(async move {
            let Self::Postgres(store) = self;
            auction_application::set_lot_watch(
                &store
                    .for_transaction(auction_observability::DatabaseTransactionOwner::PublicWatch),
                command,
            )
            .await
        })
    }
}

impl LotWatchTransaction for PostgresCompetitionTransaction {
    async fn lock_watch_lot(&mut self, lot_id: Uuid) -> Result<Uuid, LotWatchError> {
        // 与竞价一致的 world → auction → lot；不能先锁标记或 delivery 再倒取 lot。
        let row = sqlx::query("WITH target AS MATERIALIZED (
                SELECT lot.auction_id, auction.world_id FROM public.lots lot
                JOIN public.auctions auction ON auction.id = lot.auction_id WHERE lot.id = $1
            ), locked_world AS MATERIALIZED (
                SELECT w.id, w.enabled FROM public.worlds w JOIN target ON target.world_id = w.id FOR SHARE OF w
            ), locked_auction AS MATERIALIZED (
                SELECT a.id, a.status, w.enabled FROM public.auctions a
                JOIN target ON target.auction_id = a.id JOIN locked_world w ON w.id = a.world_id FOR SHARE OF a
            ) SELECT lot.auction_id, lot.status AS lot_status, a.status, a.enabled
            FROM public.lots lot JOIN locked_auction a ON a.id = lot.auction_id
            WHERE lot.id = $1 FOR UPDATE OF lot")
            .bind(lot_id).fetch_optional(&mut **self.transaction_mut()?).await.map_err(sql_error)?
            .ok_or(LotWatchError::NotFound)?;
        // 先在同一锁序下重放账本，拍卖后来隐藏不能让已接受命令失去原响应。
        // 新命令的公开可见性由 write_watch 在这些锁仍持有时复核。
        Ok(row.try_get("auction_id").map_err(sql_error)?)
    }

    async fn find_watch_command(
        &mut self,
        command_id: CommandId,
    ) -> Result<Option<StoredCommand>, LotWatchError> {
        Ok(self.find_command_inner(command_id).await?)
    }

    async fn write_watch(
        &mut self,
        command: &SetLotWatchCommand,
    ) -> Result<LotWatch, LotWatchError> {
        let transaction = self.transaction_mut()?;
        let visible: bool = sqlx::query_scalar("SELECT EXISTS (
            SELECT 1 FROM public.lots lot JOIN public.auctions a ON a.id = lot.auction_id
            JOIN public.worlds w ON w.id = a.world_id WHERE lot.id = $1
            AND (w.enabled OR a.status = 'settled')
            AND ((a.status = 'preview' AND lot.status = 'pending' AND NOT EXISTS (
                    SELECT 1 FROM public.auctions active WHERE active.world_id = a.world_id
                    AND active.status IN ('live', 'final', 'settling')))
                OR (a.status IN ('live', 'final', 'settling') AND lot.status IN ('live', 'sold', 'unsold'))
                OR (a.status = 'settled' AND lot.status IN ('sold', 'unsold'))))")
            .bind(command.lot_id).fetch_one(&mut **transaction).await.map_err(sql_error)?;
        if !visible {
            return Err(LotWatchError::NotFound);
        }
        let row = sqlx::query(
            "SELECT lot_id, watched, channel, version FROM public.lot_watches
            WHERE user_id = $1 AND lot_id = $2 FOR UPDATE",
        )
        .bind(command.user_id)
        .bind(command.lot_id)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(sql_error)?;
        let version = row.as_ref().map(map_watch).transpose()?.map_or(0, |watch| watch.version);
        if version != command.expected_version {
            return Err(LotWatchError::Conflict);
        }
        let next = version.checked_add(1).ok_or(LotWatchError::Unavailable)?;
        sqlx::query("INSERT INTO public.lot_watches (user_id, lot_id, watched, channel, version)
            VALUES ($1, $2, $3, 'mm', $4) ON CONFLICT (user_id, lot_id)
            DO UPDATE SET watched = EXCLUDED.watched, version = EXCLUDED.version, updated_at = clock_timestamp()")
            .bind(command.user_id).bind(command.lot_id).bind(command.watched).bind(next)
            .execute(&mut **transaction).await.map_err(sql_error)?;
        if !command.watched {
            invalidate(transaction, command.user_id, command.lot_id, "unwatched").await?;
        }
        audit(transaction, command.user_id, command.lot_id, "lot_watch.set",
            serde_json::json!({"command_id": command.command_id, "watched": command.watched, "version": next})).await?;
        Ok(LotWatch {
            lot_id: command.lot_id,
            watched: command.watched,
            channel: "mm".to_owned(),
            version: next,
        })
    }

    async fn store_watch_command(&mut self, command: StoredCommand) -> Result<(), LotWatchError> {
        self.insert_command_inner(&command).await.map_err(|error| match error {
            CompetitionTransactionError::CommandLedger(
                auction_application::CommandLedgerError::Conflict,
            ) => LotWatchError::Conflict,
            error => error.into(),
        })
    }
}

fn map_watch(row: &sqlx::postgres::PgRow) -> Result<LotWatch, LotWatchError> {
    let watch = LotWatch {
        lot_id: row.try_get("lot_id").map_err(sql_error)?,
        watched: row.try_get("watched").map_err(sql_error)?,
        channel: row.try_get("channel").map_err(sql_error)?,
        version: row.try_get("version").map_err(sql_error)?,
    };
    if watch.lot_id.is_nil() || watch.version <= 0 || watch.channel != "mm" {
        return Err(LotWatchError::Unavailable);
    }
    Ok(watch)
}
