//! v2 本人与 Admin 读模型：同一快照读取代管态、处分代、有效止售与撤回意愿。
//!
//! 允许动作不在这里决定：记录交给应用层的 `PrivateListingV2`，动作由领域 `seller_actions` /
//! `admin_actions` 产生。本人详情的重列资格在同一快照内交给重列写端的同一判定（`owned_relist_allowed`）。本人读只按会话用户过滤，非本人与不存在同样返回空；响应字段全部来自
//! 白名单列，不读预约码、装备 key 或原信正文（不变量 7）。

mod project;
mod query;

use auction_application::consignment::{
    AdminListingDetailV2, AdminListingQuery, AdminListingReadV2, AdminListingRecordV2,
    ConsignmentError, ConsignmentFuture, ListingDelistReason, ListingReadPage, OwnedListingReadV2,
    PrivateListingQuery, PrivateListingV2,
};
use auction_domain::UserId;
use auction_domain::consignment::ListingReason;
use sqlx::Row;
use sqlx::postgres::PgRow;
use uuid::Uuid;

use crate::database::DatabaseStore;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::deposit::repository::DepositRepository;
use crate::database::consignment::reads::private::read_conflicts;
use crate::database::consignment::reads::{snapshot, storage_error, unavailable};
use crate::database::consignment::recovery::owned_relist_allowed;
use project::private_v2;
use query::{detail_query, list_query};

async fn read_page<T>(
    store: &DatabaseStore,
    query: PrivateListingQuery,
    seller: Option<UserId>,
    admin: bool,
    project: fn(&PgRow, bool) -> Result<T, ConsignmentError>,
) -> Result<ListingReadPage<T>, ConsignmentError> {
    let (mut tx, as_of) = snapshot(store, false).await?;
    let total: i64 = list_query(as_of, query, seller, admin, false)
        .build_query_scalar()
        .fetch_one(&mut *tx)
        .await
        .map_err(storage_error)?;
    let rows = list_query(as_of, query, seller, admin, true)
        .build()
        .fetch_all(&mut *tx)
        .await
        .map_err(storage_error)?;
    tx.commit().await.map_err(storage_error)?;
    let paused = store.runtime_switches().external_access_paused();
    let items = rows.iter().map(|row| project(row, paused)).collect::<Result<_, _>>()?;
    Ok(ListingReadPage { items, total: total.cast_unsigned(), window: query.window })
}

fn admin_record(row: &PgRow, paused: bool) -> Result<AdminListingRecordV2, ConsignmentError> {
    let listing = private_v2(row, paused)?;
    let reason = row
        .try_get::<Option<String>, _>("admin_delist_reason")
        .map_err(storage_error)?
        .map(ListingDelistReason::new)
        .transpose()
        .map_err(unavailable)?;
    if listing.view.effective_reason_code() == Some(ListingReason::AdminDelisted.as_str())
        && reason.is_none()
    {
        // 现有写入器强制记录运营说明；缺少成功审计时无法可靠恢复说明。
        return Err(ConsignmentError::Unavailable);
    }
    Ok(AdminListingRecordV2 {
        listing,
        seller_user_id: row
            .try_get::<Option<Uuid>, _>("seller_user_id")
            .map_err(storage_error)?
            .map(UserId::from_uuid),
        seller_name: row.try_get("seller_name").map_err(storage_error)?,
        admin_delist_reason: reason,
    })
}

/// 本人详情；`relist_quota` 是重列写端使用的活动额度上限，没有时（没有寄售设置的装配）重列资格恒为否。
async fn owned_detail(
    store: &DatabaseStore,
    seller_user_id: UserId,
    listing_id: Uuid,
    relist_quota: Option<i64>,
) -> Result<Option<PrivateListingV2>, ConsignmentError> {
    let (mut tx, as_of) = snapshot(store, false).await?;
    let row = detail_query(as_of, listing_id, Some(seller_user_id), false)
        .build()
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage_error)?;
    // 不变量 6：重列资格与行同快照，读不出即整次读取失败，不把未知当作可重列。
    let relist_allowed = match (&row, relist_quota) {
        (Some(_), Some(quota)) => {
            owned_relist_allowed(&mut tx, seller_user_id, listing_id, quota, as_of)
                .await
                .map_err(WriteError::into_application)?
        }
        _ => false,
    };
    tx.commit().await.map_err(storage_error)?;
    let paused = store.runtime_switches().external_access_paused();
    let Some(row) = row else { return Ok(None) };
    let mut listing = private_v2(&row, paused)?;
    listing.custody.relist_allowed = relist_allowed;
    Ok(Some(listing))
}

impl OwnedListingReadV2 for DatabaseStore {
    fn list_owned_v2(
        &self,
        seller_user_id: UserId,
        query: PrivateListingQuery,
    ) -> ConsignmentFuture<'_, ListingReadPage<PrivateListingV2>> {
        Box::pin(read_page(self, query, Some(seller_user_id), false, private_v2))
    }

    fn owned_detail_v2(
        &self,
        seller_user_id: UserId,
        listing_id: Uuid,
    ) -> ConsignmentFuture<'_, Option<PrivateListingV2>> {
        Box::pin(owned_detail(self, seller_user_id, listing_id, None))
    }
}

impl AdminListingReadV2 for DatabaseStore {
    fn list_admin_v2(
        &self,
        query: AdminListingQuery,
    ) -> ConsignmentFuture<'_, ListingReadPage<AdminListingRecordV2>> {
        Box::pin(read_page(self, query.listings, query.seller_user_id, true, admin_record))
    }

    fn admin_detail_v2(
        &self,
        listing_id: Uuid,
    ) -> ConsignmentFuture<'_, Option<AdminListingDetailV2>> {
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
            let (conflicts, conflicts_truncated) = read_conflicts(&mut tx, listing_id).await?;
            tx.commit().await.map_err(storage_error)?;
            let record = admin_record(&row, self.runtime_switches().external_access_paused())?;
            Ok(Some(AdminListingDetailV2 { record, conflicts, conflicts_truncated }))
        })
    }
}

// v2 的 Public 与 Admin 装配都已持有 `DepositRead`，其端口要求挂单读取，这里委托给同一个存储。
impl OwnedListingReadV2 for DepositRepository {
    fn list_owned_v2(
        &self,
        seller_user_id: UserId,
        query: PrivateListingQuery,
    ) -> ConsignmentFuture<'_, ListingReadPage<PrivateListingV2>> {
        self.store().list_owned_v2(seller_user_id, query)
    }

    fn owned_detail_v2(
        &self,
        seller_user_id: UserId,
        listing_id: Uuid,
    ) -> ConsignmentFuture<'_, Option<PrivateListingV2>> {
        // 公开与 Admin 装配经这里读本人详情：带上重列写端同一额度上限，重列资格才可能为真。
        Box::pin(owned_detail(
            self.store(),
            seller_user_id,
            listing_id,
            Some(self.settings().max_open_per_user),
        ))
    }
}

impl AdminListingReadV2 for DepositRepository {
    fn list_admin_v2(
        &self,
        query: AdminListingQuery,
    ) -> ConsignmentFuture<'_, ListingReadPage<AdminListingRecordV2>> {
        self.store().list_admin_v2(query)
    }

    fn admin_detail_v2(
        &self,
        listing_id: Uuid,
    ) -> ConsignmentFuture<'_, Option<AdminListingDetailV2>> {
        self.store().admin_detail_v2(listing_id)
    }
}
