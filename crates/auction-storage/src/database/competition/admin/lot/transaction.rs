//! Admin lot 事务端口适配。

use super::{
    events::append_admin_lot_record_inner, mutation::update_admin_lot_inner,
    obligation::insert_withdrawn_return_obligation_inner, read::load_admin_lot_update_inner,
};
use crate::database::competition::PostgresCompetitionTransaction;
use crate::database::competition::admin::{invalid_state, map_sql_error};
use auction_application::admin::{
    AdminLotSummary, AdminLotUpdateCommand, AdminLotUpdateSnapshot, AdminLotUpdateTransaction,
};
use auction_application::core::{CompetitionTransactionError, StoredCommand};
use auction_domain::{AuctionId, LotId};
use chrono::{DateTime, Utc};
use sqlx::Row;

#[allow(clippy::manual_async_fn)]
impl AdminLotUpdateTransaction for PostgresCompetitionTransaction {
    fn find_command(
        &mut self,
        command_id: auction_domain::CommandId,
    ) -> impl Future<Output = Result<Option<StoredCommand>, CompetitionTransactionError>> + Send
    {
        async move { self.find_command_inner(command_id).await }
    }

    fn insert_command(
        &mut self,
        command: StoredCommand,
    ) -> impl Future<Output = Result<(), CompetitionTransactionError>> + Send {
        async move { self.insert_command_inner(&command).await.map(|_| ()) }
    }

    fn load_admin_lot_for_update(
        &mut self,
        lot_id: LotId,
    ) -> impl Future<Output = Result<Option<AdminLotUpdateSnapshot>, CompetitionTransactionError>> + Send
    {
        async move { load_admin_lot_update_inner(self, lot_id).await }
    }

    fn update_admin_lot(
        &mut self,
        command: &AdminLotUpdateCommand,
        now: DateTime<Utc>,
    ) -> impl Future<Output = Result<AdminLotSummary, CompetitionTransactionError>> + Send {
        async move { update_admin_lot_inner(self, command, now).await }
    }

    fn advance_admin_preview_revision(
        &mut self,
        auction_id: AuctionId,
        expected_revision: i64,
        now: DateTime<Utc>,
    ) -> impl Future<Output = Result<i64, CompetitionTransactionError>> + Send {
        async move {
            // 拍卖和 lot 使用同一时间单调性边界，避免来源时间领先时回退拍卖更新时间。
            let next_revision = expected_revision
                .checked_add(1)
                .ok_or_else(|| invalid_state("Admin lot 预览修订溢出"))?;
            let row = {
                let transaction = self.transaction_mut()?;
                sqlx::query(
                    "UPDATE public.auctions
                     SET preview_revision = $1,
                         state_version = state_version + 1,
                         updated_at = GREATEST(updated_at, $2)
                     WHERE id = $3 AND status = 'draft' AND preview_revision = $4
                     RETURNING state_version",
                )
                .bind(next_revision)
                .bind(now)
                .bind(auction_id.as_uuid())
                .bind(expected_revision)
                .fetch_optional(&mut **transaction)
                .await
                .map_err(map_sql_error)?
            };
            let Some(row) = row else {
                return Err(CompetitionTransactionError::StaleState);
            };
            row.try_get("state_version").map_err(map_sql_error)
        }
    }

    fn insert_withdrawn_return_obligation(
        &mut self,
        command: &AdminLotUpdateCommand,
        snapshot: &AdminLotUpdateSnapshot,
        now: DateTime<Utc>,
    ) -> impl Future<Output = Result<(), CompetitionTransactionError>> + Send {
        async move { insert_withdrawn_return_obligation_inner(self, command, snapshot, now).await }
    }

    fn append_admin_lot_record(
        &mut self,
        command: &AdminLotUpdateCommand,
        result: &AdminLotSummary,
        changed: bool,
        auction_state_version: i64,
        preview_revision: i64,
    ) -> impl Future<Output = Result<(), CompetitionTransactionError>> + Send {
        async move {
            append_admin_lot_record_inner(
                self,
                command,
                result,
                changed,
                auction_state_version,
                preview_revision,
            )
            .await
        }
    }
}
