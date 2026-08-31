//! 论坛主题同步快照的 PostgreSQL 一致性读取。

use auction_application::forum_world_name;
use auction_application::{
    ApplicationError, ForumAuctionSnapshot, ForumLotSnapshot, ForumSyncClaim,
};
use auction_domain::{AuctionId, SettlementWorld};
use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::super::super::PostgresStore;
use super::validation::{lot_code, parse_world, storage_error};

pub(super) async fn load_snapshot_postgres(
    store: &PostgresStore,
    claim: &ForumSyncClaim,
) -> Result<ForumAuctionSnapshot, ApplicationError> {
    use sqlx::Row;

    let mut transaction =
        store.pool().begin().await.map_err(|_| storage_error("开启论坛同步快照事务失败"))?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .execute(&mut *transaction)
        .await
        .map_err(|_| storage_error("设置论坛同步快照隔离级别失败"))?;
    let auction_row = sqlx::query(
        "SELECT auction.id, auction.world_id, auction.sequence, auction.state_version,
                auction.scheduled_open_at, auction.opened_at, auction.scheduled_end_at
         FROM public.auction_forum_sync AS sync
         JOIN public.auctions AS auction ON auction.id = sync.auction_id
         WHERE sync.auction_id = $1
           AND sync.claim_token = $2
           AND sync.claim_owner = $3
           AND sync.claim_fencing = $4
           AND sync.source_revision = $5
           AND sync.status = 'publishing'
           AND sync.lease_expires_at > CURRENT_TIMESTAMP
           AND auction.state_version = sync.source_revision",
    )
    .bind(claim.auction_id.as_uuid())
    .bind(claim.claim_token)
    .bind(&claim.worker_id)
    .bind(claim.claim_fencing)
    .bind(claim.source_revision)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|_| storage_error("读取论坛同步快照租约失败"))?;
    let Some(auction_row) = auction_row else {
        return Err(storage_error("论坛同步快照租约已失效"));
    };
    let auction_id: Uuid =
        auction_row.try_get("id").map_err(|_| storage_error("读取论坛快照拍卖标识失败"))?;
    let world_id: String =
        auction_row.try_get("world_id").map_err(|_| storage_error("读取论坛快照世界失败"))?;
    let world = parse_world(&world_id)?;
    let sequence: i64 =
        auction_row.try_get("sequence").map_err(|_| storage_error("读取论坛快照序号失败"))?;
    let state_version: i64 = auction_row
        .try_get("state_version")
        .map_err(|_| storage_error("读取论坛快照状态版本失败"))?;
    let scheduled_open_at: DateTime<Utc> = auction_row
        .try_get("scheduled_open_at")
        .map_err(|_| storage_error("读取论坛快照开启时间失败"))?;
    let opened_at: Option<DateTime<Utc>> = auction_row
        .try_get("opened_at")
        .map_err(|_| storage_error("读取论坛快照实际开启时间失败"))?;
    let scheduled_end_at: Option<DateTime<Utc>> = auction_row
        .try_get("scheduled_end_at")
        .map_err(|_| storage_error("读取论坛快照结束时间失败"))?;

    let rows = sqlx::query(
        "SELECT CASE sync.numbering_scheme
                    WHEN 'legacy_global' THEN l.lot_number
                    WHEN 'category_v1' THEN l.category_number
                END AS lot_number,
                l.status, l.start_price, l.current_price,
                l.effective_end_at, e.name AS equipment_name, e.category AS equipment_category,
                submission.seller AS seller_name, leader.username AS leader_name
         FROM public.lots AS l
         JOIN public.auction_forum_sync AS sync ON sync.auction_id = l.auction_id
         JOIN public.equipment AS e ON e.id = l.equipment_id
         LEFT JOIN public.auction_submissions AS submission ON submission.id = l.submission_id
         LEFT JOIN public.users AS leader ON leader.id = l.leader_user_id
         WHERE l.auction_id = $1
         ORDER BY l.lot_number, l.id",
    )
    .bind(auction_id)
    .fetch_all(&mut *transaction)
    .await
    .map_err(|_| storage_error("读取论坛快照 lot 失败"))?;
    let mut lots = Vec::with_capacity(rows.len());
    for row in rows {
        let lot_number: i32 =
            row.try_get("lot_number").map_err(|_| storage_error("读取论坛快照 lot 序号失败"))?;
        let category: String = row
            .try_get("equipment_category")
            .map_err(|_| storage_error("读取论坛快照装备分类失败"))?;
        let start_price: i64 =
            row.try_get("start_price").map_err(|_| storage_error("读取论坛快照起拍价失败"))?;
        let current_price: Option<i64> =
            row.try_get("current_price").map_err(|_| storage_error("读取论坛快照当前价失败"))?;
        let seller_name: Option<String> =
            row.try_get("seller_name").map_err(|_| storage_error("读取论坛快照卖家失败"))?;
        let name: String =
            row.try_get("equipment_name").map_err(|_| storage_error("读取论坛快照装备名称失败"))?;
        let status: String =
            row.try_get("status").map_err(|_| storage_error("读取论坛快照 lot 状态失败"))?;
        let effective_end_at: Option<DateTime<Utc>> = row
            .try_get("effective_end_at")
            .map_err(|_| storage_error("读取论坛快照 lot 结束时间失败"))?;
        let leader_name: Option<String> =
            row.try_get("leader_name").map_err(|_| storage_error("读取论坛快照竞价者失败"))?;
        let code = lot_code(&category, lot_number)?;
        let seller = seller_name.ok_or_else(|| storage_error("论坛快照 lot 缺少卖家"))?;
        let current_price = current_price.unwrap_or(start_price);
        if start_price <= 0 || current_price < start_price {
            return Err(storage_error("论坛快照 lot 金额无效"));
        }
        lots.push(ForumLotSnapshot {
            code,
            category,
            name,
            seller,
            start_price,
            current_price,
            status,
            current_bidder: leader_name,
            effective_end_at,
        });
    }
    transaction.commit().await.map_err(|_| storage_error("提交论坛同步快照事务失败"))?;
    let snapshot = ForumAuctionSnapshot {
        auction_id: AuctionId::from_uuid(auction_id),
        world,
        sequence,
        state_version,
        scheduled_open_at,
        opened_at,
        scheduled_end_at,
        public_url: Some(public_auction_path(world, auction_id)),
        lots,
    };
    if snapshot.auction_id != claim.auction_id
        || snapshot.state_version != claim.source_revision
        || snapshot.sequence <= 0
        || snapshot.lots.is_empty()
    {
        return Err(storage_error("论坛同步快照安全投影无效"));
    }
    Ok(snapshot)
}

fn public_auction_path(world: SettlementWorld, auction_id: Uuid) -> String {
    let world_name = forum_world_name(world);
    if world_name == "main" {
        format!("/?auction={auction_id}")
    } else {
        format!("/?auction={auction_id}&world={world_name}")
    }
}
