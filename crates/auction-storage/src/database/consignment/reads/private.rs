//! 本人与运营读取复用有效态筛选，按会话权限分别投影。

use auction_application::consignment::{
    AdminListingDetail, AdminListingQuery, AdminListingRead, AdminListingRecord, ConsignmentError,
    ConsignmentFuture, ListingAuctionConflict, ListingDelistReason, ListingReadPage,
    OwnedListingCommandReceipt, OwnedListingRead, OwnedListingRecord, PrivateListingQuery,
    SellerListingAction,
};
use auction_domain::consignment::ListingReason;
use auction_domain::{CommandId, LotId, UserId};
use auction_time::Timestamp;
use sqlx::postgres::PgRow;
use sqlx::{Postgres, QueryBuilder, Row};
use uuid::Uuid;

use crate::database::DatabaseStore;
use crate::database::consignment::eligibility::predicates::listing_sql;
use crate::database::consignment::reads::commands;
use crate::database::consignment::reads::projection::{
    EQUIPMENT_JOIN, PRIVATE_COLUMNS, WORK_JOIN, private,
};
use crate::database::consignment::reads::{page_order, snapshot, storage_error, unavailable};

const ADMIN_COLUMNS: &str = ", l.seller_user_id, s.username AS seller_name,
    a.metadata->>'admin_delist_reason' AS admin_delist_reason";

// 只从实际终结事件指向的成功审计读取说明。拒绝审计与其他挂单不能覆盖它。
const ADMIN_JOIN: &str = " LEFT JOIN LATERAL (
    SELECT a.metadata FROM public.audit_records a
    JOIN public.consignment_events e ON a.metadata->>'command_id' = e.command_id::text
    WHERE l.reason_code = 'admin_delisted' AND e.listing_id = l.id
        AND e.state_version = l.state_version AND e.reason_code = 'admin_delisted'
        AND a.entity_id = l.id::text AND a.entity_type = 'consignment_listing'
        AND a.actor_type = 'admin' AND a.action = 'consignment.admin_delist' AND a.result = 'success'
    ORDER BY a.occurred_at DESC, a.id DESC LIMIT 1
) a ON true";
const MAX_CONFLICTS: usize = 50;

fn listing_query(
    as_of: Timestamp,
    page_rows: bool,
    admin: bool,
    filter_state: bool,
    prefix: &str,
) -> QueryBuilder<Postgres> {
    let mut sql = QueryBuilder::new(prefix);
    sql.push(if page_rows { PRIVATE_COLUMNS } else { "SELECT count(*)" });
    if page_rows && admin {
        sql.push(ADMIN_COLUMNS);
    }
    sql.push(" FROM public.consignment_listings l");
    if page_rows || filter_state {
        sql.push(listing_sql!(eligibility_joins))
            .push(" CROSS JOIN (SELECT ")
            .push_bind(as_of)
            .push("::timestamptz AS at) db_clock CROSS JOIN LATERAL (SELECT ")
            .push(listing_sql!(forced_reason, "db_clock.at"))
            .push(" AS forced_reason) effective");
    }
    if page_rows {
        sql.push(EQUIPMENT_JOIN).push(WORK_JOIN);
        if admin {
            sql.push(ADMIN_JOIN);
        }
    }
    sql.push(" WHERE true");
    sql
}

fn seller_filter(sql: &mut QueryBuilder<Postgres>, seller: Option<UserId>) {
    if let Some(seller) = seller {
        sql.push(" AND l.seller_user_id = ").push_bind(seller.as_uuid());
    }
}

pub(super) fn list_query(
    as_of: Timestamp,
    query: PrivateListingQuery,
    seller: Option<UserId>,
    admin: bool,
    page_rows: bool,
    prefix: &str,
) -> QueryBuilder<Postgres> {
    let mut sql = listing_query(as_of, page_rows, admin, query.state.is_some(), prefix);
    seller_filter(&mut sql, seller);
    if let Some(world) = query.world {
        sql.push(" AND l.world_id = ").push_bind(world.as_str());
    }
    if let Some(mode) = query.mode {
        sql.push(" AND l.mode = ").push_bind(mode.as_str());
    }
    if let Some(state) = query.state {
        sql.push(
            " AND COALESCE(public.consignment_reason_state(effective.forced_reason), l.state) = ",
        )
        .push_bind(state.as_str());
    }
    if page_rows {
        page_order(&mut sql, query.window);
    }
    sql
}

fn detail_query(
    as_of: Timestamp,
    listing_id: Uuid,
    seller: Option<UserId>,
    admin: bool,
) -> QueryBuilder<Postgres> {
    let mut sql = listing_query(as_of, true, admin, false, "");
    seller_filter(&mut sql, seller);
    sql.push(" AND l.id = ").push_bind(listing_id);
    sql
}

async fn read_page<T>(
    store: &DatabaseStore,
    query: PrivateListingQuery,
    seller: Option<UserId>,
    admin: bool,
    project: fn(&PgRow, bool) -> Result<T, ConsignmentError>,
) -> Result<ListingReadPage<T>, ConsignmentError> {
    let (mut tx, as_of) = snapshot(store, false).await?;
    let total: i64 = list_query(as_of, query, seller, admin, false, "")
        .build_query_scalar()
        .fetch_one(&mut *tx)
        .await
        .map_err(storage_error)?;
    let rows = list_query(as_of, query, seller, admin, true, "")
        .build()
        .fetch_all(&mut *tx)
        .await
        .map_err(storage_error)?;
    tx.commit().await.map_err(storage_error)?;
    let paused = store.runtime_switches().external_access_paused();
    let items = rows.iter().map(|row| project(row, paused)).collect::<Result<_, _>>()?;
    Ok(ListingReadPage { items, total: total.cast_unsigned(), window: query.window })
}

fn owned(row: &PgRow, paused: bool) -> Result<OwnedListingRecord, ConsignmentError> {
    let listing = private(row, paused)?;
    let actions = if listing.view.effective_state().0.is_active() {
        vec![SellerListingAction::Reprice, SellerListingAction::Withdraw]
    } else {
        Vec::new()
    };
    OwnedListingRecord::new(listing, actions)
}

fn admin(row: &PgRow, paused: bool) -> Result<AdminListingRecord, ConsignmentError> {
    let listing = private(row, paused)?;
    let reason = row
        .try_get::<Option<String>, _>("admin_delist_reason")
        .map_err(storage_error)?
        .map(ListingDelistReason::new)
        .transpose()
        .map_err(unavailable)?;
    if listing.view.effective_state().1 == Some(ListingReason::AdminDelisted) && reason.is_none() {
        // 现有写入器强制记录运营说明。缺少成功审计时无法可靠恢复说明。
        return Err(ConsignmentError::Unavailable);
    }
    let can_delist = listing.view.effective_state().0.is_active();
    AdminListingRecord::new(
        listing,
        UserId::from_uuid(row.try_get("seller_user_id").map_err(storage_error)?),
        row.try_get("seller_name").map_err(storage_error)?,
        reason,
        can_delist,
    )
}

impl OwnedListingRead for DatabaseStore {
    fn list_owned(
        &self,
        seller_user_id: UserId,
        query: PrivateListingQuery,
    ) -> ConsignmentFuture<'_, ListingReadPage<OwnedListingRecord>> {
        Box::pin(read_page(self, query, Some(seller_user_id), false, owned))
    }

    fn owned_detail(
        &self,
        seller_user_id: UserId,
        listing_id: Uuid,
    ) -> ConsignmentFuture<'_, Option<OwnedListingRecord>> {
        Box::pin(async move {
            let (mut tx, as_of) = snapshot(self, false).await?;
            let row = detail_query(as_of, listing_id, Some(seller_user_id), false)
                .build()
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage_error)?;
            tx.commit().await.map_err(storage_error)?;
            let paused = self.runtime_switches().external_access_paused();
            row.as_ref().map(|row| owned(row, paused)).transpose()
        })
    }

    fn owned_command(
        &self,
        seller_user_id: UserId,
        command_id: CommandId,
    ) -> ConsignmentFuture<'_, Option<OwnedListingCommandReceipt>> {
        Box::pin(commands::owned_command(self, seller_user_id, command_id))
    }
}

impl AdminListingRead for DatabaseStore {
    fn list_admin(
        &self,
        query: AdminListingQuery,
    ) -> ConsignmentFuture<'_, ListingReadPage<AdminListingRecord>> {
        Box::pin(read_page(self, query.listings, query.seller_user_id, true, admin))
    }

    fn admin_detail(&self, listing_id: Uuid) -> ConsignmentFuture<'_, Option<AdminListingDetail>> {
        Box::pin(async move {
            let (mut tx, as_of) = snapshot(self, false).await?;
            let row = detail_query(as_of, listing_id, None, true)
                .build()
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage_error)?;
            let Some(row) = row else {
                tx.commit().await.map_err(storage_error)?;
                return Ok(None);
            };
            let conflicts: Vec<(Uuid, Timestamp)> = sqlx::query_as(
                "SELECT lot_id, detected_at FROM public.consignment_listing_auction_conflicts
                 WHERE listing_id = $1 ORDER BY detected_at DESC, lot_id DESC LIMIT $2",
            )
            .bind(listing_id)
            .bind(MAX_CONFLICTS as i64 + 1)
            .fetch_all(&mut *tx)
            .await
            .map_err(storage_error)?;
            tx.commit().await.map_err(storage_error)?;

            Ok(Some(AdminListingDetail {
                record: admin(&row, self.runtime_switches().external_access_paused())?,
                conflicts_truncated: conflicts.len() > MAX_CONFLICTS,
                conflicts: conflicts
                    .into_iter()
                    .take(MAX_CONFLICTS)
                    .map(|(lot_id, detected_at)| ListingAuctionConflict {
                        lot_id: LotId::from_uuid(lot_id),
                        detected_at,
                    })
                    .collect(),
            }))
        })
    }
}
