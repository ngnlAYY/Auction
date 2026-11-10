//! 论坛主题同步快照的 PostgreSQL 一致性读取。

use crate::database::storage_error_from;

use auction_application::auction::PublicEquipment;
use auction_application::core::ApplicationError;
use auction_application::forum::{
    ForumAuctionSnapshot, ForumCurrentBidSource, ForumLotSnapshot, ForumSyncClaim,
};
use auction_application::public::PublicEquipmentLinkTarget;
use auction_domain::{AuctionId, AuctionStatus, EquipmentId, SettlementWorld};
use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::validation::{lot_code, parse_world, storage_error};
use crate::database::{
    DatabaseStore, auction::competition::parse_auction_status, public::lots::map_forum_equipment,
};

pub(super) async fn load_snapshot_postgres(
    store: &DatabaseStore,
    claim: &ForumSyncClaim,
) -> Result<ForumAuctionSnapshot, ApplicationError> {
    let mut transaction = store
        .pool()
        .begin_with("BEGIN ISOLATION LEVEL REPEATABLE READ")
        .await
        .map_err(|error| storage_error_from(error, "开启论坛同步快照事务失败"))?;
    let authorized = sqlx::query(
        "SELECT 1
         FROM public.auction_forum_sync AS sync
         JOIN public.auctions AS auction ON auction.id = sync.auction_id
         WHERE sync.auction_id = $1
           AND sync.claim_token = $2
           AND sync.claim_owner = $3
           AND sync.claim_fencing = $4
           AND sync.source_revision = $5
           AND sync.status = 'publishing'
           AND sync.lease_expires_at > CURRENT_TIMESTAMP
           AND auction.status IN ('live', 'final', 'settling', 'settled')
           AND auction.state_version = sync.source_revision",
    )
    .bind(claim.auction_id.as_uuid())
    .bind(claim.claim_token)
    .bind(&claim.worker_id)
    .bind(claim.claim_fencing)
    .bind(claim.source_revision)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|error| storage_error_from(error, "读取论坛同步快照租约失败"))?;
    if authorized.is_none() {
        return Err(storage_error("论坛同步快照租约已失效"));
    }
    let snapshot = read_snapshot(&mut transaction, claim.auction_id).await?;
    transaction
        .commit()
        .await
        .map_err(|error| storage_error_from(error, "提交论坛同步快照事务失败"))?;
    if snapshot.auction_id != claim.auction_id
        || snapshot.state_version != claim.source_revision
        || snapshot.sequence <= 0
        || snapshot.lots.is_empty()
    {
        return Err(storage_error("论坛同步快照安全投影无效"));
    }
    Ok(snapshot)
}

/// 竞价命令事务完成后读取当前权威快照；只用于刷新已持久化固定楼层，不创建新主题。
pub(super) async fn load_current_snapshot_postgres(
    store: &DatabaseStore,
    auction_id: AuctionId,
) -> Result<ForumAuctionSnapshot, ApplicationError> {
    let mut transaction = store
        .pool()
        .begin_with("BEGIN ISOLATION LEVEL REPEATABLE READ")
        .await
        .map_err(|error| storage_error_from(error, "开启论坛刷新快照事务失败"))?;
    let snapshot = read_snapshot(&mut transaction, auction_id).await?;
    transaction
        .commit()
        .await
        .map_err(|error| storage_error_from(error, "提交论坛刷新快照事务失败"))?;
    if snapshot.auction_id != auction_id || snapshot.sequence <= 0 || snapshot.lots.is_empty() {
        return Err(storage_error("论坛刷新快照安全投影无效"));
    }
    Ok(snapshot)
}

async fn read_snapshot(
    transaction: &mut crate::database::TrackedTransaction<'_, sqlx::Postgres>,
    requested_auction_id: AuctionId,
) -> Result<ForumAuctionSnapshot, ApplicationError> {
    use sqlx::Row;

    let auction_row = sqlx::query(
        "SELECT auction.id, auction.world_id, auction.sequence, auction.state_version,
                auction.scheduled_open_at, auction.opened_at, auction.scheduled_end_at,
                sync.catalog_layout, auction.status AS auction_status,
                settlement.auction_id AS settled_auction_id
         FROM public.auctions AS auction
         JOIN public.auction_forum_sync AS sync ON sync.auction_id = auction.id
         LEFT JOIN public.auction_settlements AS settlement ON settlement.auction_id = auction.id
         WHERE auction.id = $1
           AND auction.status <> 'draft'
           AND (auction.status NOT IN ('preview', 'cancelled')
                OR (sync.status = 'published'
                    AND sync.thread_id IS NOT NULL
                    AND sync.catalog_post_id IS NOT NULL
                    AND sync.system_log_post_id IS NOT NULL))",
    )
    .bind(requested_auction_id.as_uuid())
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|error| storage_error_from(error, "读取论坛拍卖快照失败"))?;
    let Some(auction_row) = auction_row else {
        return Err(storage_error("论坛拍卖快照不存在"));
    };
    let auction_id: Uuid = auction_row
        .try_get("id")
        .map_err(|error| storage_error_from(error, "读取论坛快照拍卖标识失败"))?;
    let world_id: String = auction_row
        .try_get("world_id")
        .map_err(|error| storage_error_from(error, "读取论坛快照世界失败"))?;
    let world = parse_world(&world_id)?;
    let sequence: i64 = auction_row
        .try_get("sequence")
        .map_err(|error| storage_error_from(error, "读取论坛快照序号失败"))?;
    let state_version: i64 = auction_row
        .try_get("state_version")
        .map_err(|error| storage_error_from(error, "读取论坛快照状态版本失败"))?;
    let scheduled_open_at: DateTime<Utc> = auction_row
        .try_get("scheduled_open_at")
        .map_err(|error| storage_error_from(error, "读取论坛快照开启时间失败"))?;
    let opened_at: Option<DateTime<Utc>> = auction_row
        .try_get("opened_at")
        .map_err(|error| storage_error_from(error, "读取论坛快照实际开启时间失败"))?;
    let scheduled_end_at: Option<DateTime<Utc>> = auction_row
        .try_get("scheduled_end_at")
        .map_err(|error| storage_error_from(error, "读取论坛快照结束时间失败"))?;

    let auction_status: String = auction_row
        .try_get("auction_status")
        .map_err(|error| storage_error_from(error, "读取论坛拍卖状态失败"))?;
    let state = parse_auction_status(auction_status)
        .map_err(|_| storage_error("论坛拍卖状态不在闭集内"))?;
    let settled = state == AuctionStatus::Settled;
    let settled_auction_id: Option<Uuid> = auction_row
        .try_get("settled_auction_id")
        .map_err(|error| storage_error_from(error, "读取论坛结算汇总证明失败"))?;
    if settled && settled_auction_id != Some(auction_id) {
        return Err(storage_error("已结算论坛快照缺少权威结算汇总"));
    }

    // 累计领先时长仅供已结算成交项展示；活动场次刷新不能重复扫描整段竞价历史。
    let rows = sqlx::query(
        "SELECT l.id AS lot_id, e.id AS equipment_id, e.external_equipment_id,
                CASE sync.numbering_scheme
                    WHEN 'legacy_global' THEN l.lot_number
                    WHEN 'category_v1' THEN l.category_number
                END AS lot_number,
                l.status, l.start_price, l.current_price,
                l.leader_user_id, l.leader_since,
                settlement.end_price AS settlement_end_price, settlement.adjustment_ppm,
                CASE WHEN $4::boolean AND l.status = 'sold' THEN
                (SELECT COALESCE(SUM(GREATEST(0, LEAST(
          EXTRACT(EPOCH FROM (COALESCE(b.next_at, l.effective_end_at) - b.created_at)) * 1000000000,
          EXTRACT(EPOCH FROM (l.effective_end_at - b.created_at)) * 1000000000
        )))::bigint, 0)
        FROM (
            SELECT bid.bidder_user_id, bid.created_at,
                   LEAD(bid.created_at) OVER (
                       ORDER BY bid.accepted_state_version, bid.fact_index, bid.id
                   ) AS next_at
            FROM public.bids AS bid
            WHERE bid.lot_id = l.id
        ) AS b
        WHERE b.bidder_user_id = l.leader_user_id)
        ELSE NULL::bigint END AS leader_held_nanos,
                l.effective_end_at, e.name AS equipment_name, e.category AS equipment_category,
                e.attributes AS equipment_attributes,
                submission.seller AS seller_name, leader.username AS leader_name,
                current_bid.mechanism AS current_bid_mechanism,
                forum_command.post_number AS current_bid_post_number,
                forum_command.thread_id AS current_bid_thread_id,
                forum_command.post_id AS current_bid_post_id
         FROM public.lots AS l
         JOIN public.auction_forum_sync AS sync ON sync.auction_id = l.auction_id
         JOIN public.equipment AS e ON e.id = l.equipment_id
         LEFT JOIN public.auction_submissions AS submission ON submission.id = l.submission_id
         LEFT JOIN public.users AS leader ON leader.id = l.leader_user_id
         LEFT JOIN public.lot_settlements AS settlement ON settlement.lot_id = l.id
         LEFT JOIN LATERAL (
             SELECT bid.command_id, bid.mechanism
             FROM public.bids AS bid
             WHERE bid.lot_id = l.id
               AND bid.bidder_user_id = l.leader_user_id
               AND bid.amount = l.current_price
             ORDER BY bid.accepted_state_version DESC, bid.fact_index DESC, bid.id DESC
             LIMIT 1
         ) AS current_bid ON TRUE
         LEFT JOIN public.auction_forum_bid_commands AS forum_command
           ON forum_command.command_id = current_bid.command_id
          AND forum_command.auction_id = l.auction_id
          AND forum_command.command_kind = 'bid'
         WHERE l.auction_id = $1
           AND (($2 AND l.status = 'pending')
                OR ($3 AND l.status = 'withdrawn')
                OR (NOT $2 AND NOT $3 AND l.status IN ('live', 'sold', 'unsold')))
         ORDER BY l.lot_number, l.id",
    )
    .bind(auction_id)
    .bind(state == AuctionStatus::Preview)
    .bind(state == AuctionStatus::Cancelled)
    .bind(settled)
    .fetch_all(&mut **transaction)
    .await
    .map_err(|error| storage_error_from(error, "读取论坛快照 lot 失败"))?;
    let mut lots = Vec::with_capacity(rows.len());
    for row in rows {
        let lot_number: i32 = row
            .try_get("lot_number")
            .map_err(|error| storage_error_from(error, "读取论坛快照 lot 序号失败"))?;
        let lot_id: Uuid = row
            .try_get("lot_id")
            .map_err(|error| storage_error_from(error, "读取论坛快照 lot 标识失败"))?;
        let equipment_id: Uuid = row
            .try_get("equipment_id")
            .map_err(|error| storage_error_from(error, "读取论坛快照装备标识失败"))?;
        let external_equipment_id: String = row
            .try_get("external_equipment_id")
            .map_err(|error| storage_error_from(error, "读取论坛快照外部装备标识失败"))?;
        let category: String = row
            .try_get("equipment_category")
            .map_err(|error| storage_error_from(error, "读取论坛快照装备分类失败"))?;
        let start_price: i64 = row
            .try_get("start_price")
            .map_err(|error| storage_error_from(error, "读取论坛快照起拍价失败"))?;
        let current_price: Option<i64> = row
            .try_get("current_price")
            .map_err(|error| storage_error_from(error, "读取论坛快照当前价失败"))?;
        let seller_name: Option<String> = row
            .try_get("seller_name")
            .map_err(|error| storage_error_from(error, "读取论坛快照卖家失败"))?;
        let name: String = row
            .try_get("equipment_name")
            .map_err(|error| storage_error_from(error, "读取论坛快照装备名称失败"))?;
        let attributes: sqlx::types::Json<serde_json::Value> = row
            .try_get("equipment_attributes")
            .map_err(|error| storage_error_from(error, "读取论坛快照装备属性失败"))?;
        let status: String = row
            .try_get("status")
            .map_err(|error| storage_error_from(error, "读取论坛快照 lot 状态失败"))?;
        let effective_end_at: Option<DateTime<Utc>> = row
            .try_get("effective_end_at")
            .map_err(|error| storage_error_from(error, "读取论坛快照 lot 结束时间失败"))?;
        let leader_name: Option<String> = row
            .try_get("leader_name")
            .map_err(|error| storage_error_from(error, "读取论坛快照竞价者失败"))?;
        let current_bid_mechanism: Option<String> = row
            .try_get("current_bid_mechanism")
            .map_err(|error| storage_error_from(error, "读取论坛快照竞价来源失败"))?;
        let current_bid_post_number: Option<i32> = row
            .try_get("current_bid_post_number")
            .map_err(|error| storage_error_from(error, "读取论坛快照竞价楼层失败"))?;
        let current_bid_thread_id: Option<String> = row
            .try_get("current_bid_thread_id")
            .map_err(|error| storage_error_from(error, "读取论坛快照竞价主题失败"))?;
        let current_bid_post_id: Option<String> = row
            .try_get("current_bid_post_id")
            .map_err(|error| storage_error_from(error, "读取论坛快照竞价帖子失败"))?;
        let current_bid_source = match current_bid_mechanism.as_deref() {
            Some("proxy") => Some(ForumCurrentBidSource::Proxy),
            Some("ordinary") => {
                match (current_bid_post_number, current_bid_thread_id, current_bid_post_id) {
                    (Some(post_number), Some(thread_id), Some(post_id)) => {
                        Some(ForumCurrentBidSource::Forum { post_number, thread_id, post_id })
                    }
                    _ => None,
                }
            }
            Some(_) => return Err(storage_error("论坛快照竞价机制不在闭集内")),
            None => None,
        };
        let code = lot_code(&category, lot_number)?;
        let seller = seller_name.ok_or_else(|| storage_error("论坛快照 lot 缺少卖家"))?;
        let current_price = current_price.unwrap_or(start_price);
        if start_price <= 0 || current_price < start_price {
            return Err(storage_error("论坛快照 lot 金额无效"));
        }
        let equipment =
            map_forum_equipment(&world_id, &external_equipment_id, &name, &category, attributes.0)?;
        let identity = &equipment.attributes;
        let equipment_quality =
            identity.get("quality").and_then(serde_json::Value::as_str).unwrap_or_default();
        let equipment_type =
            identity.get("type").and_then(serde_json::Value::as_str).unwrap_or_default();
        let equipment_prefix =
            identity.get("prefix").and_then(serde_json::Value::as_str).unwrap_or_default();
        let equipment_suffix =
            identity.get("suffix").and_then(serde_json::Value::as_str).unwrap_or_default();
        let settlement = super::settlement::project(&row, settled)?;
        lots.push(ForumLotSnapshot {
            code,
            category,
            name,
            equipment_quality: equipment_quality.to_owned(),
            equipment_type: equipment_type.to_owned(),
            equipment_prefix: equipment_prefix.to_owned(),
            equipment_suffix: equipment_suffix.to_owned(),
            equipment_level: equipment.level.filter(|level| *level > 0),
            primary_attribute_initials: equipment.primary_attribute_initials(),
            avg_roll_tenths: average_roll_tenths(&equipment),
            seller,
            start_price,
            current_price,
            status,
            current_bidder: leader_name,
            current_bid_source,
            effective_end_at,
            settlement,
            equipment_link_target: PublicEquipmentLinkTarget {
                lot_id,
                equipment_id: EquipmentId::from_uuid(equipment_id),
                world_id: world.as_str().to_owned(),
                projected_game_equipment_url: projected_game_equipment_url(
                    world,
                    &external_equipment_id,
                ),
            },
        });
    }
    let layout: String = auction_row
        .try_get("catalog_layout")
        .map_err(|error| storage_error_from(error, "读取论坛冻结目录布局失败"))?;
    let snapshot = ForumAuctionSnapshot {
        catalog_layout: super::super::catalog::parse_layout(&layout)?,
        auction_id: AuctionId::from_uuid(auction_id),
        world,
        sequence,
        state_version,
        state,
        scheduled_open_at,
        opened_at,
        scheduled_end_at,
        public_url: Some(public_auction_path(world, auction_id)),
        lots,
    };
    Ok(snapshot)
}

fn public_auction_path(world: SettlementWorld, auction_id: Uuid) -> String {
    let world_name = world.as_str();
    if world_name == "main" {
        format!("/?auction={auction_id}")
    } else {
        format!("/?auction={auction_id}&world={world_name}")
    }
}

fn projected_game_equipment_url(world: SettlementWorld, external_equipment_id: &str) -> String {
    let prefix = match world {
        SettlementWorld::Main => "https://hentaiverse.org/equip/",
        SettlementWorld::Isekai => "https://hentaiverse.org/isekai/equip/",
    };
    format!("{prefix}{external_equipment_id}")
}

fn average_roll_tenths(equipment: &PublicEquipment) -> Option<i64> {
    equipment
        .avg_roll
        .filter(|value| value.is_finite() && (0.0..=100.0).contains(value))
        .map(|value| (value * 10.0).round() as i64)
}
