//! Admin lot 状态与起拍价变更。

use crate::database::auction::competition::PostgresCompetitionTransaction;
use crate::database::auction::competition::mapping::map_sql_error;
use auction_application::admin::{AdminLotSummary, AdminLotUpdateCommand};
use auction_application::core::CompetitionTransactionError;
use chrono::{DateTime, Utc};

pub(super) async fn update_admin_lot_inner(
    transaction: &mut PostgresCompetitionTransaction,
    command: &AdminLotUpdateCommand,
    now: DateTime<Utc>,
) -> Result<AdminLotSummary, CompetitionTransactionError> {
    let start_price = command.start_price;
    // 邮件来源时间或时钟回拨可能令既有更新时间领先本机；修改必须保持时间单调，否则撤拍后的安全
    // 投影会因 updated_at < created_at 被拒绝并回滚整笔事务。
    let updated = {
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            "UPDATE public.lots
             SET status = CASE WHEN $1 = 'withdraw' THEN 'withdrawn' ELSE 'pending' END,
                 start_price = CASE WHEN $1 = 'set_start_price' THEN $2 ELSE start_price END,
                 current_price = CASE WHEN $1 = 'set_start_price' THEN $2 ELSE current_price END,
                 state_version = state_version + 1,
                 updated_at = GREATEST(updated_at, $3)
             WHERE id = $4 AND auction_id = (
                 SELECT auction_id FROM public.lots WHERE id = $4
             ) AND status = 'pending' AND state_version = $5
             RETURNING id",
        )
        .bind(command.action.as_str())
        .bind(start_price)
        .bind(now)
        .bind(command.lot_id.as_uuid())
        .bind(command.expected_state_version)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(map_sql_error)?
    };
    if updated.is_none() {
        return Err(CompetitionTransactionError::StaleState);
    }
    super::read::load_admin_lot_update_inner(transaction, command.lot_id)
        .await?
        .map(|snapshot| snapshot.lot)
        .ok_or(CompetitionTransactionError::NotFound)
}
