//! Admin lot 锁内安全投影读取。

use super::super::map_admin_lot_row;
use crate::database::competition::PostgresCompetitionTransaction;
use crate::database::competition::admin::{invalid_state, map_sql_error};
use auction_application::{AdminLotUpdateSnapshot, CompetitionTransactionError};
use auction_domain::LotId;
use sqlx::Row;
use uuid::Uuid;

pub(super) async fn load_admin_lot_update_inner(
    transaction: &mut PostgresCompetitionTransaction,
    lot_id: LotId,
) -> Result<Option<AdminLotUpdateSnapshot>, CompetitionTransactionError> {
    let scope = {
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            "SELECT a.id AS auction_id, a.world_id
             FROM public.lots AS l
             INNER JOIN public.auctions AS a ON a.id = l.auction_id
             WHERE l.id = $1",
        )
        .bind(lot_id.as_uuid())
        .fetch_optional(&mut **transaction)
        .await
        .map_err(map_sql_error)?
    };
    let Some(scope) = scope else {
        return Ok(None);
    };
    let auction_id: Uuid = scope.try_get("auction_id").map_err(map_sql_error)?;
    let world_id: String = scope.try_get("world_id").map_err(map_sql_error)?;

    {
        let transaction = transaction.transaction_mut()?;
        let world_exists: Option<String> =
            sqlx::query_scalar("SELECT id FROM public.worlds WHERE id = $1 FOR UPDATE")
                .bind(&world_id)
                .fetch_optional(&mut **transaction)
                .await
                .map_err(map_sql_error)?;
        if world_exists.is_none() {
            return Ok(None);
        }
        let auction_exists: Option<Uuid> =
            sqlx::query_scalar("SELECT id FROM public.auctions WHERE id = $1 FOR UPDATE")
                .bind(auction_id)
                .fetch_optional(&mut **transaction)
                .await
                .map_err(map_sql_error)?;
        if auction_exists.is_none() {
            return Ok(None);
        }
    }

    let row = {
        let transaction = transaction.transaction_mut()?;
        sqlx::query(
            "SELECT
                a.status AS auction_status, a.state_version AS auction_state_version,
                a.preview_revision,
                COALESCE((a.policy_snapshot->'settlement'->>'version')::integer, 0)
                    AS policy_version,
                l.id, l.auction_id, l.equipment_id, l.submission_id,
                l.category_number AS lot_number, l.status,
                l.start_price, COALESCE(l.current_price, l.start_price) AS current_price,
                l.leader_user_id, l.effective_end_at, l.slow_due_at, l.slow_generation,
                l.slow_authorization_id, l.state_version, l.created_at, l.updated_at,
                e.external_equipment_id, e.name AS equipment_name, e.category AS equipment_category,
                e.attributes AS equipment_attributes, e.collected_at AS equipment_collected_at,
                s.seller, s.source_kind AS submission_source_kind,
                s.source_reference AS submission_source_reference, s.mailbox_key,
                s.external_mail_id, s.received_at AS submission_received_at
             FROM public.lots AS l
             INNER JOIN public.auctions AS a ON a.id = l.auction_id
             INNER JOIN public.equipment AS e ON e.id = l.equipment_id
             LEFT JOIN public.auction_submissions AS s ON s.id = l.submission_id
             WHERE l.id = $1 AND a.id = $2
             FOR UPDATE OF l",
        )
        .bind(lot_id.as_uuid())
        .bind(auction_id)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(map_sql_error)?
    };
    let Some(row) = row else {
        return Ok(None);
    };
    let auction_status: String = row.try_get("auction_status").map_err(map_sql_error)?;
    let auction_state_version: i64 = row.try_get("auction_state_version").map_err(map_sql_error)?;
    let preview_revision: i64 = row.try_get("preview_revision").map_err(map_sql_error)?;
    let policy_version: i32 = row.try_get("policy_version").map_err(map_sql_error)?;
    let lot = map_admin_lot_row(row)?;
    let snapshot = AdminLotUpdateSnapshot {
        lot,
        auction_status,
        world_id,
        auction_state_version,
        preview_revision,
        policy_version,
    };
    if !snapshot.validate() {
        return Err(invalid_state("Admin lot 锁内投影校验失败"));
    }
    Ok(Some(snapshot))
}
