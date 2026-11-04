//! 论坛回帖主题、lot、身份和代理的 PostgreSQL 只读查询。

#[cfg(feature = "postgres")]
use crate::database::storage_error_from;

#[cfg(feature = "postgres")]
use auction_application::{
    ApplicationError, ForumActiveProxy, ForumCommandOutcome, ForumCommandStatus, ForumLotTarget,
    ForumReplyPhase, ForumReplyTopic,
};
#[cfg(feature = "postgres")]
use auction_domain::{AuctionId, LotId, UserId};
#[cfg(feature = "postgres")]
use uuid::Uuid;

#[cfg(feature = "postgres")]
use super::super::super::PostgresStore;
#[cfg(feature = "postgres")]
use super::validation::{
    MAX_FIXED_POST_ID_BYTES, MAX_THREAD_ID_BYTES, parse_world, storage_error, valid_decimal_id,
};

#[cfg(feature = "postgres")]
pub(super) async fn claim_published_topics_postgres(
    store: &PostgresStore,
    worker_id: &str,
    now: chrono::DateTime<chrono::Utc>,
    lease_until: chrono::DateTime<chrono::Utc>,
    available_worlds: &[auction_domain::SettlementWorld],
    limit: u16,
) -> Result<Vec<ForumReplyTopic>, ApplicationError> {
    use sqlx::Row;

    let available_worlds = available_worlds
        .iter()
        .copied()
        .map(auction_domain::SettlementWorld::as_str)
        .collect::<Vec<_>>();
    let mut transaction = store
        .pool()
        .begin()
        .await
        .map_err(|error| storage_error_from(error, "开启论坛回帖领取事务失败"))?;
    let rows = sqlx::query(
        "SELECT sync.auction_id, auction.world_id, auction.status, sync.thread_id,
                sync.catalog_post_id, sync.system_log_post_id,
                sync.title, sync.body, sync.system_log_body, sync.content_revision,
                sync.processed_through_post_number,
                CASE WHEN sync.content_claim_token IS NOT NULL THEN 'write_unknown'
                     ELSE sync.content_last_error_code END AS content_last_error_code,
                sync.catalog_page_count, sync.catalog_layout
         FROM public.auction_forum_sync AS sync
         JOIN public.auctions AS auction ON auction.id = sync.auction_id
         WHERE sync.status = 'published'
           AND auction.world_id = ANY($3::text[])
           AND sync.thread_id IS NOT NULL
           AND sync.catalog_post_id IS NOT NULL
           AND sync.system_log_post_id IS NOT NULL
           AND sync.title IS NOT NULL
           AND sync.body IS NOT NULL
           AND sync.system_log_body IS NOT NULL
           AND sync.content_revision > 0
           AND (auction.status IN ('preview', 'live', 'final', 'settling')
                OR (auction.status IN ('settled', 'cancelled') AND sync.terminal_refresh_enabled
                    AND sync.terminal_synced_version IS NULL))
           AND sync.content_next_scan_at <= $1
           AND (
               sync.content_claim_token IS NULL
               OR sync.content_lease_expires_at IS NULL
               OR sync.content_lease_expires_at <= $1
           )
         ORDER BY sync.content_next_scan_at ASC, sync.auction_id ASC
         LIMIT $2
         FOR UPDATE OF sync SKIP LOCKED",
    )
    .bind(now)
    .bind(i64::from(limit))
    .bind(&available_worlds)
    .fetch_all(&mut *transaction)
    .await
    .map_err(|error| storage_error_from(error, "读取已发布论坛主题失败"))?;

    let mut topics = Vec::with_capacity(rows.len());
    for row in rows {
        let auction_id: Uuid = row
            .try_get("auction_id")
            .map_err(|error| storage_error_from(error, "读取论坛回帖拍卖标识失败"))?;
        let world_id: String = row
            .try_get("world_id")
            .map_err(|error| storage_error_from(error, "读取论坛回帖世界失败"))?;
        let auction_status: String = row
            .try_get("status")
            .map_err(|error| storage_error_from(error, "读取论坛回帖拍卖状态失败"))?;
        let thread_id: String = row
            .try_get("thread_id")
            .map_err(|error| storage_error_from(error, "读取论坛回帖主题标识失败"))?;
        let catalog_post_id: String = row
            .try_get("catalog_post_id")
            .map_err(|error| storage_error_from(error, "读取论坛目录帖子标识失败"))?;
        let system_log_post_id: String = row
            .try_get("system_log_post_id")
            .map_err(|error| storage_error_from(error, "读取论坛系统日志帖子标识失败"))?;
        let title: String = row
            .try_get("title")
            .map_err(|error| storage_error_from(error, "读取论坛主题标题失败"))?;
        let catalog_body: String = row
            .try_get("body")
            .map_err(|error| storage_error_from(error, "读取论坛目录正文失败"))?;
        let system_log_body: String = row
            .try_get("system_log_body")
            .map_err(|error| storage_error_from(error, "读取论坛系统日志正文失败"))?;
        let content_revision: i64 = row
            .try_get("content_revision")
            .map_err(|error| storage_error_from(error, "读取论坛固定楼层版本失败"))?;
        let processed_through_post_number: i32 = row
            .try_get("processed_through_post_number")
            .map_err(|error| storage_error_from(error, "读取论坛已处理楼层失败"))?;
        let content_last_error_code: Option<String> = row
            .try_get("content_last_error_code")
            .map_err(|error| storage_error_from(error, "读取论坛固定楼层错误码失败"))?;
        let layout: String = row
            .try_get("catalog_layout")
            .map_err(|error| storage_error_from(error, "读取论坛冻结目录布局失败"))?;
        let catalog_layout = super::super::catalog::parse_layout(&layout)?;
        let catalog_page_count: i32 = row
            .try_get("catalog_page_count")
            .map_err(|error| storage_error_from(error, "读取论坛目录页数失败"))?;
        let catalog_continuation =
            super::super::catalog::read_pages(&mut transaction, auction_id, catalog_page_count)
                .await?;
        let content_claim_token = Uuid::new_v4();
        // 旧 claim 已过期但可能在外部 POST 后崩溃；重新领取时同事务固定未知门禁。
        let claimed = sqlx::query(
            "UPDATE public.auction_forum_sync
             SET content_last_error_at = CASE WHEN content_claim_token IS NOT NULL
                     THEN GREATEST(content_last_error_at, $6) ELSE content_last_error_at END,
                 content_last_error_code = CASE WHEN content_claim_token IS NOT NULL
                     THEN 'write_unknown' ELSE content_last_error_code END,
                 content_claim_token = $2,
                 content_claim_owner = $3,
                 content_claim_fencing = content_claim_fencing + 1,
                 content_lease_expires_at = $4
             WHERE auction_id = $1
               AND status = 'published'
               AND content_revision = $5
             RETURNING content_claim_fencing",
        )
        .bind(auction_id)
        .bind(content_claim_token)
        .bind(worker_id)
        .bind(lease_until)
        .bind(content_revision)
        .bind(now)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|error| storage_error_from(error, "写入论坛固定楼层内容租约失败"))?;
        let Some(claimed) = claimed else {
            return Err(storage_error("论坛固定楼层内容租约在事务内变化"));
        };
        let content_claim_fencing: i64 = claimed
            .try_get("content_claim_fencing")
            .map_err(|error| storage_error_from(error, "读取论坛固定楼层 fencing 失败"))?;
        let world = parse_world(&world_id)?;
        if auction_id.is_nil()
            || !valid_decimal_id(&thread_id, MAX_THREAD_ID_BYTES)
            || !valid_decimal_id(&catalog_post_id, MAX_FIXED_POST_ID_BYTES)
            || !valid_decimal_id(&system_log_post_id, MAX_FIXED_POST_ID_BYTES)
            || catalog_post_id == system_log_post_id
            || title.is_empty()
            || title.len() > 512
            || catalog_body.is_empty()
            || catalog_body.len() > 65_531
            || system_log_body.is_empty()
            || system_log_body.len() > 65_531
            || content_revision <= 0
            || processed_through_post_number <= catalog_page_count
            || !super::super::catalog::valid_pages(
                &catalog_post_id,
                &system_log_post_id,
                &catalog_continuation,
            )
        {
            return Err(storage_error("论坛回帖主题投影无效"));
        }
        topics.push(ForumReplyTopic {
            auction_id: AuctionId::from_uuid(auction_id),
            world,
            reply_phase: ForumReplyPhase::from_auction_status(&auction_status),
            thread_id,
            catalog_post_id,
            catalog_layout,
            catalog_continuation,
            system_log_post_id,
            title,
            catalog_body,
            system_log_body,
            content_revision,
            confirmed_terminal_version: None,
            processed_through_post_number,
            content_claim_token,
            content_claim_owner: worker_id.to_owned(),
            content_claim_fencing,
            content_lease_until: lease_until,
            content_last_error_code,
        });
    }
    transaction
        .commit()
        .await
        .map_err(|error| storage_error_from(error, "提交论坛回帖领取事务失败"))?;
    Ok(topics)
}

#[cfg(feature = "postgres")]
pub(super) async fn reply_cursor_postgres(
    store: &PostgresStore,
    auction_id: AuctionId,
    thread_id: &str,
) -> Result<Option<auction_application::ForumTopicCursor>, ApplicationError> {
    use sqlx::Row;

    // 楼层最高的已导入命令就是增量扫描的起点；pending 也计入，因为重扫该页仍然安全。
    let row = sqlx::query(
        "SELECT post_id, post_number
         FROM public.auction_forum_bid_commands
         WHERE auction_id = $1 AND thread_id = $2 AND post_number IS NOT NULL
         ORDER BY post_number DESC, post_id DESC
         LIMIT 1",
    )
    .bind(auction_id.as_uuid())
    .bind(thread_id)
    .fetch_optional(store.pool())
    .await
    .map_err(|error| storage_error_from(error, "读取论坛回帖导入游标失败"))?;
    let Some(row) = row else { return Ok(None) };
    let post_id: String = row
        .try_get("post_id")
        .map_err(|error| storage_error_from(error, "读取论坛回帖游标帖子标识失败"))?;
    let post_number: i32 = row
        .try_get("post_number")
        .map_err(|error| storage_error_from(error, "读取论坛回帖游标楼层失败"))?;
    let post_number = u32::try_from(post_number).map_err(|error| {
        let _ = error;
        storage_error("论坛回帖游标楼层无效")
    })?;
    let cursor = auction_application::ForumTopicCursor { post_id, post_number };
    cursor.validate().map_err(storage_error)?;
    Ok(Some(cursor))
}

#[cfg(feature = "postgres")]
pub(super) async fn earliest_live_lot_deadline_postgres(
    store: &PostgresStore,
    auction_id: AuctionId,
) -> Result<Option<chrono::DateTime<chrono::Utc>>, ApplicationError> {
    sqlx::query_scalar::<_, Option<chrono::DateTime<chrono::Utc>>>(
        "SELECT MIN(lot.effective_end_at)
         FROM public.lots AS lot
         WHERE lot.auction_id = $1 AND lot.status = 'live'",
    )
    .bind(auction_id.as_uuid())
    .fetch_one(store.pool())
    .await
    .map_err(|error| storage_error_from(error, "读取论坛回帖最早 lot 截止失败"))
}

#[cfg(feature = "postgres")]
pub(super) async fn reply_commands_open_postgres(
    store: &PostgresStore,
    auction_id: AuctionId,
) -> Result<bool, ApplicationError> {
    sqlx::query_scalar::<_, bool>(
        "SELECT status IN ('live', 'final') FROM public.auctions WHERE id = $1",
    )
    .bind(auction_id.as_uuid())
    .fetch_one(store.pool())
    .await
    .map_err(|error| storage_error_from(error, "读取论坛回帖接收状态失败"))
}

#[cfg(feature = "postgres")]
pub(super) async fn command_outcomes_postgres(
    store: &PostgresStore,
    auction_id: AuctionId,
    thread_id: &str,
) -> Result<Vec<ForumCommandOutcome>, ApplicationError> {
    use sqlx::Row;

    let rows = sqlx::query(
        "SELECT post_id, post_number, line_number, lot_code, command_kind, status,
                last_error_code, last_error_detail
         FROM public.auction_forum_bid_commands
         WHERE auction_id = $1 AND thread_id = $2
         ORDER BY post_id, line_number, lot_code, command_kind",
    )
    .bind(auction_id.as_uuid())
    .bind(thread_id)
    .fetch_all(store.pool())
    .await
    .map_err(|error| storage_error_from(error, "读取论坛命令处理结果失败"))?;
    let mut outcomes = Vec::with_capacity(rows.len());
    for row in rows {
        let status: String = row
            .try_get("status")
            .map_err(|error| storage_error_from(error, "读取论坛命令状态失败"))?;
        let status = match status.as_str() {
            "pending" => ForumCommandStatus::Pending,
            "applied" => ForumCommandStatus::Applied,
            "rejected" => ForumCommandStatus::Rejected,
            _ => return Err(storage_error("论坛命令状态不在闭集内")),
        };
        let post_number: Option<i32> = row
            .try_get("post_number")
            .map_err(|error| storage_error_from(error, "读取论坛命令楼层号失败"))?;
        if post_number.is_some_and(|value| value < 3) {
            return Err(storage_error("论坛命令楼层号不在用户回复范围内"));
        }
        outcomes.push(ForumCommandOutcome {
            post_id: row
                .try_get("post_id")
                .map_err(|error| storage_error_from(error, "读取论坛命令帖子标识失败"))?,
            post_number,
            line_number: row
                .try_get("line_number")
                .map_err(|error| storage_error_from(error, "读取论坛命令行号失败"))?,
            lot_code: row
                .try_get("lot_code")
                .map_err(|error| storage_error_from(error, "读取论坛命令 lot 编码失败"))?,
            command_kind: row
                .try_get("command_kind")
                .map_err(|error| storage_error_from(error, "读取论坛命令类型失败"))?,
            status,
            error_code: row
                .try_get("last_error_code")
                .map_err(|error| storage_error_from(error, "读取论坛命令错误码失败"))?,
            safe_detail: row
                .try_get("last_error_detail")
                .map_err(|error| storage_error_from(error, "读取论坛命令安全详情失败"))?,
        });
    }
    Ok(outcomes)
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
    .map_err(|error| storage_error_from(error, "读取论坛回帖 lot 映射失败"))?;

    let mut targets = Vec::with_capacity(rows.len());
    for row in rows {
        let lot_uuid: Uuid = row
            .try_get("id")
            .map_err(|error| storage_error_from(error, "读取论坛回帖 lot 标识失败"))?;
        let lot_number: i32 = row
            .try_get("lot_number")
            .map_err(|error| storage_error_from(error, "读取论坛回帖 lot 序号失败"))?;
        let category: String = row
            .try_get("category")
            .map_err(|error| storage_error_from(error, "读取论坛回帖 lot 分类失败"))?;
        let code = super::super::sync::lot_code(&category, lot_number)?;
        targets.push(ForumLotTarget { code, lot_id: LotId::from_uuid(lot_uuid) });
    }
    if targets.is_empty() {
        return Err(storage_error("论坛回帖拍卖缺少 lot 映射"));
    }
    Ok(targets)
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
    .map_err(|error| storage_error_from(error, "读取论坛活动代理失败"))?;
    row.map(|row| {
        let authorization_id: Uuid = row
            .try_get("authorization_id")
            .map_err(|error| storage_error_from(error, "读取论坛活动代理标识失败"))?;
        if authorization_id.is_nil() {
            return Err(storage_error("论坛活动代理标识无效"));
        }
        Ok(ForumActiveProxy { authorization_id: ProxyAuthorizationId::from_uuid(authorization_id) })
    })
    .transpose()
}
