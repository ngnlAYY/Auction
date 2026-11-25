//! 公开 v2 目录与详情：展示中的非寄售广告、待寄寄售件，以及代管上架的 `available` 件。
//!
//! 可见性沿用 v1 的有效态谓词（强制终结、过期、准入关闭的行不可见）；`available` 另要求有代管
//! position 与处分代，缺失时整行不可见（失败关闭）。`purchasable` 由世界购买能力门计算：
//! 验证器签收清单恒假的本期，它恒为 false（R5-C10）。公开响应不含预约码、装备 key 与代管内部 ID。

use auction_application::consignment::{
    CONSIGNMENT_PURCHASE_VALIDATORS_SIGNED, ConsignmentError, ConsignmentFuture, ListingReadPage,
    ListingWorld, PublicCatalogDetailV2, PublicCatalogEntryV2, PublicCatalogReadV2,
    PublicListingQuery,
};
use auction_domain::SettlementWorld;
use auction_domain::consignment::{
    ListingEquipmentIdentity, ListingMode, ListingPrice, ListingState, ListingVersion, SeasonState,
};
use auction_time::Timestamp;
use sqlx::postgres::PgRow;
use sqlx::{PgConnection, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use crate::database::DatabaseStore;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::deposit::primitives::{
    current_season, mail_readiness, season_state,
};
use crate::database::consignment::eligibility::predicates::listing_sql;
use crate::database::consignment::reads::projection::equipment;
use crate::database::consignment::reads::{page_order, snapshot, storage_error, unavailable};

const COLUMNS: &str = "SELECT l.id, l.world_id, l.external_equipment_id, l.state_version, l.state,
        l.mode, l.price, l.updated_at, l.expires_at, s.username AS seller_name,
        pos.current_generation AS generation,
        p.name, p.category, p.alt_name, p.level_kind, p.level_number, p.tradeable, p.observed_at";

/// 世界的购买能力门（R5-C10）：验证器已签收 ∧ 收件协议就绪 ∧ 资金账户就绪 ∧ 季态可购。
///
/// `signed` 取自应用层的签收清单；本期清单全为 false，所以恒为 false，翻转清单是单独的提交。
pub(crate) fn purchase_ready(
    world: ListingWorld,
    mail_ready: bool,
    season: SeasonState,
    funding_ready: bool,
) -> bool {
    gate(CONSIGNMENT_PURCHASE_VALIDATORS_SIGNED.signed(world), mail_ready, season, funding_ready)
}

/// 四个条件的合取；与签收清单分开，便于单测每个条件。
const fn gate(signed: bool, mail_ready: bool, season: SeasonState, funding_ready: bool) -> bool {
    signed
        && mail_ready
        && funding_ready
        && matches!(season, SeasonState::NotSeasonal | SeasonState::Open)
}

/// 粗粒度的资金能力读：存在 ready 账户且没有未核清的现金退件阻断。仅用于展示；
/// 真正的购买事务仍做完整的同源资金预检（购买与 Sale 冻结同一份事实）。
pub(crate) async fn funding_ready(conn: &mut PgConnection) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM public.settlement_funding_accounts WHERE status = 'ready')
            AND NOT EXISTS (SELECT 1 FROM public.consignment_cash_return_blocks
                            WHERE state = 'open')",
    )
    .fetch_one(conn)
    .await
}

/// 两个世界此刻是否可购买：(main, isekai)。
async fn world_gates(
    conn: &mut PgConnection,
    now: Timestamp,
) -> Result<(bool, bool), ConsignmentError> {
    let funding = funding_ready(&mut *conn).await.map_err(storage_error)?;
    let main = mail_readiness(&mut *conn, SettlementWorld::Main)
        .await
        .map_err(WriteError::into_application)?
        .is_some_and(|row| row.fully_ready(now));
    let isekai = mail_readiness(&mut *conn, SettlementWorld::Isekai)
        .await
        .map_err(WriteError::into_application)?
        .is_some_and(|row| row.fully_ready(now));
    let window = current_season(&mut *conn, now).await.map_err(WriteError::into_application)?;
    let state = season_state(SettlementWorld::Isekai, window.as_ref(), now);
    Ok((
        purchase_ready(ListingWorld::Main, main, SeasonState::NotSeasonal, funding),
        purchase_ready(ListingWorld::Isekai, isekai, state, funding),
    ))
}

fn visible(query: &mut QueryBuilder<Postgres>, as_of: Timestamp) {
    query
        .push(" FROM public.consignment_listings l ")
        .push(listing_sql!(eligibility_joins))
        .push(
            " JOIN public.consignment_snapshots p
                ON p.listing_id = l.id AND p.id = l.current_snapshot_id
              LEFT JOIN public.consignment_custody_positions pos ON pos.listing_id = l.id
              CROSS JOIN (SELECT ",
        )
        .push_bind(as_of)
        .push("::timestamptz AS at) db_clock WHERE ")
        .push(
            "(l.state IN ('displayed', 'awaiting_deposit', 'available')
              AND (l.state <> 'available'
                   OR (l.mode = 'consigned' AND pos.state = 'in_custody'
                       AND pos.current_generation IS NOT NULL))
              AND ",
        )
        .push(listing_sql!(effective_active, "db_clock.at"))
        .push(
            " AND EXISTS (SELECT 1 FROM public.consignment_snapshots snapshot
                          WHERE snapshot.listing_id = l.id AND snapshot.id = l.current_snapshot_id))",
        );
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

fn list_query(
    select: &str,
    as_of: Timestamp,
    query: &PublicListingQuery,
) -> QueryBuilder<Postgres> {
    let mut sql = QueryBuilder::new(select);
    visible(&mut sql, as_of);
    filters(&mut sql, query);
    sql
}

fn entry(
    row: &PgRow,
    details: &auction_application::consignment::ListingEquipmentDetails,
    gates: (bool, bool),
) -> Result<PublicCatalogEntryV2, ConsignmentError> {
    let state: ListingState =
        row.try_get::<&str, _>("state").map_err(storage_error)?.parse().map_err(unavailable)?;
    let mode: ListingMode =
        row.try_get::<&str, _>("mode").map_err(storage_error)?.parse().map_err(unavailable)?;
    let identity = ListingEquipmentIdentity::new(
        row.try_get("world_id").map_err(storage_error)?,
        row.try_get("external_equipment_id").map_err(storage_error)?,
    )
    .map_err(unavailable)?;
    let label = details.label();
    if !label.tradeable {
        return Err(ConsignmentError::Unavailable);
    }
    let generation: Option<i64> = row.try_get("generation").map_err(storage_error)?;
    // available 必有处分代；其余态不带，形状不符失败关闭。
    if (state == ListingState::Available) != generation.is_some() {
        return Err(ConsignmentError::Unavailable);
    }
    let world_ready = match identity.world_id() {
        "main" => gates.0,
        "isekai" => gates.1,
        _ => return Err(ConsignmentError::Unavailable),
    };
    Ok(PublicCatalogEntryV2 {
        listing_id: row.try_get("id").map_err(storage_error)?,
        version: ListingVersion::new(
            row.try_get::<i64, _>("state_version").map_err(storage_error)?.cast_unsigned(),
        )
        .map_err(unavailable)?,
        state,
        mode,
        identity,
        name: label.name.clone(),
        category: label.category.clone(),
        level: label.level,
        price: ListingPrice::new(row.try_get("price").map_err(storage_error)?)
            .map_err(unavailable)?,
        seller_name: row.try_get("seller_name").map_err(storage_error)?,
        updated_at: row.try_get("updated_at").map_err(storage_error)?,
        expires_at: row.try_get("expires_at").map_err(storage_error)?,
        generation: generation.map(i64::cast_unsigned),
        purchasable: state == ListingState::Available && world_ready,
    })
}

async fn read_page(
    store: &DatabaseStore,
    query: PublicListingQuery,
) -> Result<ListingReadPage<PublicCatalogEntryV2>, ConsignmentError> {
    let (mut tx, as_of) = snapshot(store, true).await?;
    let total: i64 = list_query("SELECT count(*)", as_of, &query)
        .build_query_scalar()
        .fetch_one(&mut *tx)
        .await
        .map_err(storage_error)?;
    let mut sql = list_query(COLUMNS, as_of, &query);
    page_order(&mut sql, query.window);
    let rows = sql.build().fetch_all(&mut *tx).await.map_err(storage_error)?;
    let gates = world_gates(&mut tx, as_of).await?;
    tx.commit().await.map_err(storage_error)?;
    let items = rows
        .iter()
        .map(|row| entry(row, &equipment(row, false)?, gates))
        .collect::<Result<_, _>>()?;
    Ok(ListingReadPage { items, total: total.cast_unsigned(), window: query.window })
}

impl PublicCatalogReadV2 for DatabaseStore {
    fn list_public_v2(
        &self,
        query: PublicListingQuery,
    ) -> ConsignmentFuture<'_, ListingReadPage<PublicCatalogEntryV2>> {
        Box::pin(read_page(self, query))
    }

    fn public_detail_v2(
        &self,
        listing_id: Uuid,
    ) -> ConsignmentFuture<'_, Option<PublicCatalogDetailV2>> {
        Box::pin(async move {
            let (mut tx, as_of) = snapshot(self, true).await?;
            let mut sql = QueryBuilder::new(COLUMNS);
            sql.push(", p.stat_groups");
            visible(&mut sql, as_of);
            sql.push(" AND l.id = ").push_bind(listing_id);
            let row = sql.build().fetch_optional(&mut *tx).await.map_err(storage_error)?;
            let gates = world_gates(&mut tx, as_of).await?;
            tx.commit().await.map_err(storage_error)?;
            row.map(|row| {
                let equipment = equipment(&row, true)?;
                Ok(PublicCatalogDetailV2 { entry: entry(&row, &equipment, gates)?, equipment })
            })
            .transpose()
        })
    }
}

#[cfg(test)]
mod tests {
    use auction_application::consignment::CONSIGNMENT_PURCHASE_VALIDATORS_SIGNED;
    use auction_domain::consignment::SeasonState;

    use super::gate;

    /// 四个条件缺一不可；预停、关闭与未知季态都不可购。
    #[test]
    fn gate_requires_every_condition() {
        assert!(gate(true, true, SeasonState::Open, true));
        assert!(gate(true, true, SeasonState::NotSeasonal, true));
        assert!(!gate(false, true, SeasonState::Open, true));
        assert!(!gate(true, false, SeasonState::Open, true));
        assert!(!gate(true, true, SeasonState::Open, false));
        for state in [SeasonState::Preclose, SeasonState::Closed, SeasonState::Unknown] {
            assert!(!gate(true, true, state, true), "{state:?}");
        }
    }

    /// 样本与签收是单独提交：本期签收清单必须全为 false，生产恒不可购买。
    #[test]
    fn validators_are_unsigned_in_this_phase() {
        const {
            assert!(!CONSIGNMENT_PURCHASE_VALIDATORS_SIGNED.main);
            assert!(!CONSIGNMENT_PURCHASE_VALIDATORS_SIGNED.isekai);
        }
    }
}
