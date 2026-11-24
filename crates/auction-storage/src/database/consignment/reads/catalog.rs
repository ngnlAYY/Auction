//! 公开列表与详情复用资格谓词，数据库仅返回公开字段。

use auction_application::consignment::{
    ConsignmentError, ConsignmentFuture, ListingReadPage, ListingRegistrationAvailability,
    PublicListingDetail, PublicListingQuery, PublicListingRead, PublicListingState,
    PublicListingSummary, RegistrationReplayCrypto,
};
use auction_domain::consignment::{ListingEquipmentIdentity, ListingPrice, ListingVersion};
use auction_time::Timestamp;
use sqlx::postgres::PgRow;
use sqlx::{Postgres, QueryBuilder, Row};
use uuid::Uuid;

use crate::database::DatabaseStore;
use crate::database::consignment::eligibility::predicates::listing_sql;
use crate::database::consignment::reads::availability;
use crate::database::consignment::reads::projection::equipment;
use crate::database::consignment::reads::{page_order, snapshot, storage_error, unavailable};

const PUBLIC_COLUMNS: &str =
    "SELECT l.id, l.world_id, l.external_equipment_id, l.state_version, l.state, l.price,
            l.updated_at, l.expires_at, s.username AS seller_name,
            p.name, p.category, p.alt_name, p.level_kind, p.level_number, p.tradeable, p.observed_at";

fn visible(query: &mut QueryBuilder<Postgres>, as_of: Timestamp) {
    query
        .push(" FROM public.consignment_listings l ")
        .push(listing_sql!(eligibility_joins))
        .push(
            " JOIN public.consignment_snapshots p
                ON p.listing_id = l.id AND p.id = l.current_snapshot_id
                CROSS JOIN (SELECT ",
        )
        .push_bind(as_of)
        .push("::timestamptz AS at) db_clock WHERE ")
        .push(listing_sql!(public_visible, "db_clock.at"));
}

fn filters(sql: &mut QueryBuilder<Postgres>, query: &PublicListingQuery) {
    if let Some(world) = query.world {
        sql.push(" AND l.world_id = ").push_bind(world.as_str());
    }
    if let Some(mode) = query.mode {
        sql.push(" AND l.mode = ").push_bind(mode.as_str());
    }
    if let Some(category) = &query.category {
        sql.push(" AND p.category = ").push_bind(category.as_str().to_owned());
    }
    if let Some(name) = &query.name {
        let name = name.as_str();
        sql.push(" AND (strpos(lower(p.name), lower(")
            .push_bind(name.to_owned())
            .push(")) > 0 OR strpos(lower(p.alt_name), lower(")
            .push_bind(name.to_owned())
            .push(")) > 0)");
    }
}

pub(super) fn list_query(
    select: &str,
    as_of: Timestamp,
    query: &PublicListingQuery,
) -> QueryBuilder<Postgres> {
    let mut sql = QueryBuilder::new(select);
    visible(&mut sql, as_of);
    filters(&mut sql, query);
    sql
}

#[cfg(feature = "test-db")]
pub(super) fn page_select() -> &'static str {
    PUBLIC_COLUMNS
}

fn summary(
    row: &PgRow,
    details: &auction_application::consignment::ListingEquipmentDetails,
) -> Result<PublicListingSummary, ConsignmentError> {
    let state = match row.try_get::<&str, _>("state").map_err(storage_error)? {
        "displayed" => PublicListingState::Displayed,
        "awaiting_deposit" => PublicListingState::AwaitingDeposit,
        _ => return Err(ConsignmentError::Unavailable),
    };
    let label = details.label();
    if !label.tradeable {
        return Err(ConsignmentError::Unavailable);
    }
    Ok(PublicListingSummary {
        listing_id: row.try_get("id").map_err(storage_error)?,
        version: ListingVersion::new(
            row.try_get::<i64, _>("state_version").map_err(storage_error)?.cast_unsigned(),
        )
        .map_err(unavailable)?,
        state,
        identity: ListingEquipmentIdentity::new(
            row.try_get("world_id").map_err(storage_error)?,
            row.try_get("external_equipment_id").map_err(storage_error)?,
        )
        .map_err(unavailable)?,
        name: label.name.clone(),
        category: label.category.clone(),
        level: label.level,
        price: ListingPrice::new(row.try_get("price").map_err(storage_error)?)
            .map_err(unavailable)?,
        seller_name: row.try_get("seller_name").map_err(storage_error)?,
        updated_at: row.try_get("updated_at").map_err(storage_error)?,
        expires_at: row.try_get("expires_at").map_err(storage_error)?,
    })
}

async fn count(
    tx: &mut sqlx::PgConnection,
    as_of: Timestamp,
    query: &PublicListingQuery,
) -> Result<u64, ConsignmentError> {
    let total: i64 = list_query("SELECT count(*)", as_of, query)
        .build_query_scalar()
        .fetch_one(tx)
        .await
        .map_err(storage_error)?;
    Ok(total.cast_unsigned())
}

async fn items(
    tx: &mut sqlx::PgConnection,
    as_of: Timestamp,
    query: &PublicListingQuery,
) -> Result<Vec<PublicListingSummary>, ConsignmentError> {
    let mut sql = list_query(PUBLIC_COLUMNS, as_of, query);
    page_order(&mut sql, query.window);
    let rows = sql.build().fetch_all(tx).await.map_err(storage_error)?;
    rows.iter().map(|row| summary(row, &equipment(row, false)?)).collect()
}

pub(super) async fn read_page(
    store: &DatabaseStore,
    query: PublicListingQuery,
    between_count_and_page: impl AsyncFnOnce(u64),
) -> Result<ListingReadPage<PublicListingSummary>, ConsignmentError> {
    let (mut tx, as_of) = snapshot(store, true).await?;
    let total = count(&mut tx, as_of, &query).await?;
    between_count_and_page(total).await;
    let items = items(&mut tx, as_of, &query).await?;
    tx.commit().await.map_err(storage_error)?;
    Ok(ListingReadPage { items, total, window: query.window })
}

impl PublicListingRead for DatabaseStore {
    fn list_public(
        &self,
        query: PublicListingQuery,
    ) -> ConsignmentFuture<'_, ListingReadPage<PublicListingSummary>> {
        Box::pin(read_page(self, query, async |_| {}))
    }

    fn public_detail(
        &self,
        listing_id: Uuid,
    ) -> ConsignmentFuture<'_, Option<PublicListingDetail>> {
        Box::pin(async move {
            let (mut tx, as_of) = snapshot(self, true).await?;
            let mut sql = QueryBuilder::new(PUBLIC_COLUMNS);
            sql.push(", p.stat_groups");
            visible(&mut sql, as_of);
            sql.push(" AND l.id = ").push_bind(listing_id);
            let row = sql.build().fetch_optional(&mut *tx).await.map_err(storage_error)?;
            tx.commit().await.map_err(storage_error)?;
            row.map(|row| {
                let equipment = equipment(&row, true)?;
                Ok(PublicListingDetail { summary: summary(&row, &equipment)?, equipment })
            })
            .transpose()
        })
    }

    fn registration_availability<'a>(
        &'a self,
        replay_crypto: &'a dyn RegistrationReplayCrypto,
    ) -> ConsignmentFuture<'a, ListingRegistrationAvailability> {
        Box::pin(availability::registration(self, replay_crypto))
    }
}
