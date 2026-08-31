//! 论坛回帖主题、lot、身份和代理的 PostgreSQL 只读查询。

#[cfg(feature = "postgres")]
use auction_application::forum_world_name;
#[cfg(feature = "postgres")]
use auction_application::{
    ApplicationError, ForumActiveProxy, ForumBidderIdentity, ForumLotTarget, ForumReplyTopic,
};
#[cfg(feature = "postgres")]
use auction_domain::{AuctionId, LotId, SettlementWorld, UserId};
#[cfg(feature = "postgres")]
use uuid::Uuid;

#[cfg(feature = "postgres")]
use super::super::super::PostgresStore;
#[cfg(feature = "postgres")]
use super::validation::{
    MAX_DISPLAY_NAME_BYTES, MAX_EXTERNAL_ACCOUNT_BYTES, MAX_THREAD_ID_BYTES, parse_world,
    storage_error, valid_decimal_id, valid_text,
};

#[cfg(feature = "postgres")]
pub(super) async fn list_published_topics_postgres(
    store: &PostgresStore,
    limit: u16,
) -> Result<Vec<ForumReplyTopic>, ApplicationError> {
    use sqlx::Row;

    let rows = sqlx::query(
        "SELECT sync.auction_id, auction.world_id, sync.thread_id
         FROM public.auction_forum_sync AS sync
         JOIN public.auctions AS auction ON auction.id = sync.auction_id
         WHERE sync.status = 'published'
           AND sync.thread_id IS NOT NULL
           AND auction.status IN ('live', 'final')
         ORDER BY sync.updated_at ASC, sync.auction_id ASC
         LIMIT $1",
    )
    .bind(i64::from(limit))
    .fetch_all(store.pool())
    .await
    .map_err(|_| storage_error("读取已发布论坛主题失败"))?;

    let mut topics = Vec::with_capacity(rows.len());
    for row in rows {
        let auction_id: Uuid =
            row.try_get("auction_id").map_err(|_| storage_error("读取论坛回帖拍卖标识失败"))?;
        let world_id: String =
            row.try_get("world_id").map_err(|_| storage_error("读取论坛回帖世界失败"))?;
        let thread_id: String =
            row.try_get("thread_id").map_err(|_| storage_error("读取论坛回帖主题标识失败"))?;
        let world = parse_world(&world_id)?;
        if auction_id.is_nil() || !valid_decimal_id(&thread_id, MAX_THREAD_ID_BYTES) {
            return Err(storage_error("论坛回帖主题投影无效"));
        }
        topics.push(ForumReplyTopic {
            auction_id: AuctionId::from_uuid(auction_id),
            world,
            thread_id,
        });
    }
    Ok(topics)
}

#[cfg(feature = "postgres")]
pub(super) async fn lot_targets_postgres(
    store: &PostgresStore,
    auction_id: AuctionId,
) -> Result<Vec<ForumLotTarget>, ApplicationError> {
    use sqlx::Row;

    let rows = sqlx::query(
        "SELECT lot.id,
                CASE sync.numbering_scheme
                    WHEN 'legacy_global' THEN lot.lot_number
                    WHEN 'category_v1' THEN lot.category_number
                END AS lot_number,
                equipment.category
         FROM public.lots AS lot
         JOIN public.auction_forum_sync AS sync
           ON sync.auction_id = lot.auction_id AND sync.status = 'published'
         JOIN public.equipment AS equipment ON equipment.id = lot.equipment_id
         WHERE lot.auction_id = $1
           AND lot.status IN ('live', 'sold')
         ORDER BY lot.lot_number ASC, lot.id ASC",
    )
    .bind(auction_id.as_uuid())
    .fetch_all(store.pool())
    .await
    .map_err(|_| storage_error("读取论坛回帖 lot 映射失败"))?;

    let mut targets = Vec::with_capacity(rows.len());
    for row in rows {
        let lot_uuid: Uuid =
            row.try_get("id").map_err(|_| storage_error("读取论坛回帖 lot 标识失败"))?;
        let lot_number: i32 =
            row.try_get("lot_number").map_err(|_| storage_error("读取论坛回帖 lot 序号失败"))?;
        let category: String =
            row.try_get("category").map_err(|_| storage_error("读取论坛回帖 lot 分类失败"))?;
        let code = super::super::sync::lot_code(&category, lot_number)?;
        targets.push(ForumLotTarget { code, lot_id: LotId::from_uuid(lot_uuid) });
    }
    if targets.is_empty() {
        return Err(storage_error("论坛回帖拍卖缺少 lot 映射"));
    }
    Ok(targets)
}

#[cfg(feature = "postgres")]
pub(super) async fn resolve_bidder_postgres(
    store: &PostgresStore,
    world: SettlementWorld,
    external_account_id: &str,
    display_name: &str,
) -> Result<ForumBidderIdentity, ApplicationError> {
    use sqlx::Row;

    let row = sqlx::query(
        "SELECT user_id, external_account_id, display_name
         FROM public.forum_bidder_identities
         WHERE world_id = $1
           AND provider = 'ehentai'
           AND status = 'active'
           AND lower(external_account_id) = lower($2)",
    )
    .bind(forum_world_name(world))
    .bind(external_account_id)
    .fetch_optional(store.pool())
    .await
    .map_err(|_| storage_error("读取论坛身份映射失败"))?;
    let Some(row) = row else {
        return Err(storage_error("论坛身份未登记或已禁用"));
    };
    let user_uuid: Uuid =
        row.try_get("user_id").map_err(|_| storage_error("读取论坛身份用户标识失败"))?;
    let registered_account_id: String = row
        .try_get("external_account_id")
        .map_err(|_| storage_error("读取论坛身份外部标识失败"))?;
    let registered_display_name: String =
        row.try_get("display_name").map_err(|_| storage_error("读取论坛身份显示名失败"))?;
    if user_uuid.is_nil()
        || !valid_text(&registered_account_id, MAX_EXTERNAL_ACCOUNT_BYTES)
        || !valid_text(&registered_display_name, MAX_DISPLAY_NAME_BYTES)
        || display_name.trim().is_empty()
    {
        return Err(storage_error("论坛身份映射违反安全边界"));
    }
    Ok(ForumBidderIdentity {
        user_id: UserId::from_uuid(user_uuid),
        external_account_id: registered_account_id,
        display_name: registered_display_name,
    })
}

#[cfg(feature = "postgres")]
pub(super) async fn active_proxy_postgres(
    store: &PostgresStore,
    lot_id: LotId,
    user_id: UserId,
) -> Result<Option<ForumActiveProxy>, ApplicationError> {
    use auction_domain::ProxyAuthorizationId;
    use sqlx::Row;

    let row = sqlx::query(
        "SELECT authorization_id
         FROM public.bid_proxy_authorizations
         WHERE lot_id = $1
           AND user_id = $2
           AND status IN ('active', 'final_fast')
         ORDER BY authorization_sequence DESC, authorization_id DESC
         LIMIT 1",
    )
    .bind(lot_id.as_uuid())
    .bind(user_id.as_uuid())
    .fetch_optional(store.pool())
    .await
    .map_err(|_| storage_error("读取论坛活动代理失败"))?;
    row.map(|row| {
        let authorization_id: Uuid = row
            .try_get("authorization_id")
            .map_err(|_| storage_error("读取论坛活动代理标识失败"))?;
        if authorization_id.is_nil() {
            return Err(storage_error("论坛活动代理标识无效"));
        }
        Ok(ForumActiveProxy { authorization_id: ProxyAuthorizationId::from_uuid(authorization_id) })
    })
    .transpose()
}
