//! PostgreSQL 论坛回帖导入存储。
//!
//! 论坛命令先落入本地账本，再由 Worker 调用现有竞价端口。该模块只负责身份、lot 和
//! 幂等事实的持久化，不直接改变竞价状态；SQLite Lite 明确不提供这条完整档能力。

#[cfg(feature = "postgres")]
use auction_application::ForumCommandStatus;
#[cfg(feature = "postgres")]
use auction_application::forum_world_name;
use auction_application::{
    ApplicationError, ApplicationFuture, ForumActiveProxy, ForumBidCommandRecord,
    ForumBidderIdentity, ForumCommandStart, ForumLotTarget, ForumReplyStore, ForumReplyTopic,
    RepositoryError,
};
#[cfg(feature = "postgres")]
use auction_domain::ProxyAuthorizationId;
use auction_domain::{AuctionId, LotId, SettlementWorld, UserId};
use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::super::DatabaseStore;

const MAX_REPLY_BATCH_SIZE: u16 = 100;
const MAX_EXTERNAL_ACCOUNT_BYTES: usize = 256;
const MAX_DISPLAY_NAME_BYTES: usize = 256;
const MAX_THREAD_ID_BYTES: usize = 64;
const MAX_POST_ID_BYTES: usize = 256;
const MAX_LOT_CODE_BYTES: usize = 16;
const MAX_ACTOR_BYTES: usize = 256;
const MAX_COMMAND_KIND_BYTES: usize = 32;
const MAX_ERROR_CODE_BYTES: usize = 128;
const MAX_ERROR_DETAIL_BYTES: usize = 2048;

impl ForumReplyStore for DatabaseStore {
    fn list_published_topics(&self, limit: u16) -> ApplicationFuture<'_, Vec<ForumReplyTopic>> {
        Box::pin(async move {
            if limit == 0 || limit > MAX_REPLY_BATCH_SIZE {
                return Err(storage_error("论坛回帖主题批量大小无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    list_published_topics_postgres(store, limit).await
                }
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(_) => Err(capability_error()),
            }
        })
    }

    fn lot_targets(&self, auction_id: AuctionId) -> ApplicationFuture<'_, Vec<ForumLotTarget>> {
        Box::pin(async move {
            if auction_id.is_nil() {
                return Err(storage_error("论坛回帖拍卖标识无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => lot_targets_postgres(store, auction_id).await,
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(_) => Err(capability_error()),
            }
        })
    }

    fn resolve_bidder(
        &self,
        world: SettlementWorld,
        external_account_id: String,
        display_name: String,
    ) -> ApplicationFuture<'_, ForumBidderIdentity> {
        Box::pin(async move {
            #[cfg(feature = "sqlite")]
            let _ = world;
            if !valid_text(&external_account_id, MAX_EXTERNAL_ACCOUNT_BYTES)
                || !valid_text(&display_name, MAX_DISPLAY_NAME_BYTES)
            {
                return Err(storage_error("论坛回帖身份参数无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    resolve_bidder_postgres(store, world, &external_account_id, &display_name).await
                }
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(_) => Err(capability_error()),
            }
        })
    }

    fn active_proxy(
        &self,
        lot_id: LotId,
        user_id: UserId,
    ) -> ApplicationFuture<'_, Option<ForumActiveProxy>> {
        Box::pin(async move {
            if lot_id.is_nil() || user_id.is_nil() {
                return Err(storage_error("论坛回帖代理查询标识无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    active_proxy_postgres(store, lot_id, user_id).await
                }
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(_) => Err(capability_error()),
            }
        })
    }

    fn begin_command(
        &self,
        record: ForumBidCommandRecord,
    ) -> ApplicationFuture<'_, ForumCommandStart> {
        Box::pin(async move {
            if !valid_command_record(&record) {
                return Err(storage_error("论坛回帖命令账本参数无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => begin_command_postgres(store, record).await,
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(_) => Err(capability_error()),
            }
        })
    }

    fn finish_command(
        &self,
        command_id: Uuid,
        status: String,
        error_code: String,
        safe_detail: String,
    ) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            if command_id.is_nil()
                || !matches!(status.as_str(), "applied" | "rejected")
                || !valid_optional_text(&error_code, MAX_ERROR_CODE_BYTES)
                || !valid_optional_text(&safe_detail, MAX_ERROR_DETAIL_BYTES)
            {
                return Err(storage_error("论坛回帖命令完成参数无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    finish_command_postgres(store, command_id, &status, &error_code, &safe_detail)
                        .await
                }
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(_) => Err(capability_error()),
            }
        })
    }

    fn mark_scanned(
        &self,
        world: SettlementWorld,
        scanned_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            #[cfg(feature = "sqlite")]
            let _ = world;
            if scanned_at.timestamp_nanos_opt().is_none() {
                return Err(storage_error("论坛回帖扫描时间无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    mark_scanned_postgres(store, world, scanned_at).await
                }
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(_) => Err(capability_error()),
            }
        })
    }
}

#[cfg(feature = "postgres")]
async fn list_published_topics_postgres(
    store: &super::super::PostgresStore,
    limit: u16,
) -> Result<Vec<ForumReplyTopic>, ApplicationError> {
    use sqlx::Row;

    let rows = sqlx::query(
        "SELECT sync.auction_id, auction.world_id, sync.thread_id
         FROM public.auction_forum_sync AS sync
         JOIN public.auctions AS auction ON auction.id = sync.auction_id
         WHERE sync.status = 'published'
           AND sync.thread_id IS NOT NULL
           AND auction.status IN ('preview', 'live', 'final', 'settling', 'settled')
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
async fn lot_targets_postgres(
    store: &super::super::PostgresStore,
    auction_id: AuctionId,
) -> Result<Vec<ForumLotTarget>, ApplicationError> {
    use sqlx::Row;

    let rows = sqlx::query(
        "SELECT lot.id, lot.lot_number, equipment.category
         FROM public.lots AS lot
         JOIN public.equipment AS equipment ON equipment.id = lot.equipment_id
         WHERE lot.auction_id = $1
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
        let code = super::sync::lot_code(&category, lot_number)?;
        targets.push(ForumLotTarget { code, lot_id: LotId::from_uuid(lot_uuid) });
    }
    if targets.is_empty() {
        return Err(storage_error("论坛回帖拍卖缺少 lot 映射"));
    }
    Ok(targets)
}

#[cfg(feature = "postgres")]
async fn resolve_bidder_postgres(
    store: &super::super::PostgresStore,
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
async fn active_proxy_postgres(
    store: &super::super::PostgresStore,
    lot_id: LotId,
    user_id: UserId,
) -> Result<Option<ForumActiveProxy>, ApplicationError> {
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

#[cfg(feature = "postgres")]
async fn begin_command_postgres(
    store: &super::super::PostgresStore,
    record: ForumBidCommandRecord,
) -> Result<ForumCommandStart, ApplicationError> {
    use sqlx::Row;

    let mut transaction =
        store.pool().begin().await.map_err(|_| storage_error("开启论坛命令账本事务失败"))?;
    let inserted = sqlx::query(
        "INSERT INTO public.auction_forum_bid_commands
            (command_id, auction_id, world_id, thread_id, post_id, line_number, lot_code,
             actor, command_kind, amount, status, attempt_count)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, 'pending', 1)
         ON CONFLICT (command_id) DO NOTHING",
    )
    .bind(record.command_id)
    .bind(record.auction_id.as_uuid())
    .bind(forum_world_name(record.world))
    .bind(&record.thread_id)
    .bind(&record.post_id)
    .bind(record.line_number)
    .bind(&record.lot_code)
    .bind(&record.actor)
    .bind(&record.command_kind)
    .bind(record.amount)
    .execute(&mut *transaction)
    .await
    .map_err(|_| storage_error("写入论坛命令账本失败"))?;
    if inserted.rows_affected() == 1 {
        transaction.commit().await.map_err(|_| storage_error("提交论坛命令账本事务失败"))?;
        return Ok(ForumCommandStart { status: ForumCommandStatus::Pending, should_run: true });
    }

    let row = sqlx::query(
        "SELECT auction_id, world_id, thread_id, post_id, line_number, lot_code,
                actor, command_kind, amount, status
         FROM public.auction_forum_bid_commands
         WHERE command_id = $1
         FOR UPDATE",
    )
    .bind(record.command_id)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|_| storage_error("读取既有论坛命令账本失败"))?;
    let Some(row) = row else {
        return Err(storage_error("论坛命令账本冲突记录不存在"));
    };
    let existing_auction_id: Uuid =
        row.try_get("auction_id").map_err(|_| storage_error("读取论坛命令拍卖标识失败"))?;
    let existing_world: String =
        row.try_get("world_id").map_err(|_| storage_error("读取论坛命令世界失败"))?;
    let existing_thread_id: String =
        row.try_get("thread_id").map_err(|_| storage_error("读取论坛命令主题标识失败"))?;
    let existing_post_id: String =
        row.try_get("post_id").map_err(|_| storage_error("读取论坛命令帖子标识失败"))?;
    let existing_line_number: i32 =
        row.try_get("line_number").map_err(|_| storage_error("读取论坛命令行号失败"))?;
    let existing_lot_code: String =
        row.try_get("lot_code").map_err(|_| storage_error("读取论坛命令 lot 编码失败"))?;
    let existing_actor: String =
        row.try_get("actor").map_err(|_| storage_error("读取论坛命令操作者失败"))?;
    let existing_kind: String =
        row.try_get("command_kind").map_err(|_| storage_error("读取论坛命令类型失败"))?;
    let existing_amount: i64 =
        row.try_get("amount").map_err(|_| storage_error("读取论坛命令金额失败"))?;
    let stored_status: String =
        row.try_get("status").map_err(|_| storage_error("读取论坛命令状态失败"))?;
    let same = existing_auction_id == record.auction_id.as_uuid()
        && existing_world == forum_world_name(record.world)
        && existing_thread_id == record.thread_id
        && existing_post_id == record.post_id
        && existing_line_number == record.line_number
        && existing_lot_code == record.lot_code
        && existing_actor == record.actor
        && existing_kind == record.command_kind
        && existing_amount == record.amount;
    if !same {
        return Err(storage_error("论坛命令标识与既有负载冲突"));
    }
    let (status, should_run) = match stored_status.as_str() {
        "pending" => {
            sqlx::query(
                "UPDATE public.auction_forum_bid_commands
                 SET attempt_count = attempt_count + 1, updated_at = CURRENT_TIMESTAMP
                 WHERE command_id = $1",
            )
            .bind(record.command_id)
            .execute(&mut *transaction)
            .await
            .map_err(|_| storage_error("更新论坛命令重试次数失败"))?;
            (ForumCommandStatus::Pending, true)
        }
        "applied" => (ForumCommandStatus::Applied, false),
        "rejected" => (ForumCommandStatus::Rejected, false),
        _ => return Err(storage_error("论坛命令账本状态不在闭集内")),
    };
    transaction.commit().await.map_err(|_| storage_error("提交既有论坛命令账本事务失败"))?;
    Ok(ForumCommandStart { status, should_run })
}

#[cfg(feature = "postgres")]
async fn finish_command_postgres(
    store: &super::super::PostgresStore,
    command_id: Uuid,
    status: &str,
    error_code: &str,
    safe_detail: &str,
) -> Result<(), ApplicationError> {
    use sqlx::Row;

    let result = sqlx::query(
        "UPDATE public.auction_forum_bid_commands
         SET status = $2,
             last_error_code = NULLIF($3, ''),
             last_error_detail = NULLIF($4, ''),
             updated_at = CURRENT_TIMESTAMP
         WHERE command_id = $1 AND status = 'pending'",
    )
    .bind(command_id)
    .bind(status)
    .bind(error_code)
    .bind(safe_detail)
    .execute(store.pool())
    .await
    .map_err(|_| storage_error("完成论坛命令账本失败"))?;
    if result.rows_affected() == 1 {
        return Ok(());
    }
    let current: Option<String> =
        sqlx::query("SELECT status FROM public.auction_forum_bid_commands WHERE command_id = $1")
            .bind(command_id)
            .fetch_optional(store.pool())
            .await
            .map_err(|_| storage_error("读取论坛命令完成状态失败"))?
            .map(|row| row.try_get("status"))
            .transpose()
            .map_err(|_| storage_error("解析论坛命令完成状态失败"))?;
    match current.as_deref() {
        Some(current) if current == status => Ok(()),
        Some(_) => Err(storage_error("论坛命令已由其他状态完成")),
        None => Err(storage_error("论坛命令账本记录不存在")),
    }
}

#[cfg(feature = "postgres")]
async fn mark_scanned_postgres(
    store: &super::super::PostgresStore,
    world: SettlementWorld,
    scanned_at: DateTime<Utc>,
) -> Result<(), ApplicationError> {
    let result = sqlx::query(
        "UPDATE public.auction_forum_reply_cursors
         SET last_scan_at = GREATEST(last_scan_at, $2),
             state_version = state_version + 1,
             updated_at = CURRENT_TIMESTAMP
         WHERE scope = $1",
    )
    .bind(forum_world_name(world))
    .bind(scanned_at)
    .execute(store.pool())
    .await
    .map_err(|_| storage_error("更新论坛回帖扫描游标失败"))?;
    if result.rows_affected() != 1 {
        return Err(storage_error("论坛回帖扫描游标不存在"));
    }
    Ok(())
}

fn valid_command_record(record: &ForumBidCommandRecord) -> bool {
    !record.command_id.is_nil()
        && !record.auction_id.is_nil()
        && valid_text(&record.thread_id, MAX_THREAD_ID_BYTES)
        && valid_text(&record.post_id, MAX_POST_ID_BYTES)
        && record.line_number > 0
        && valid_text(&record.lot_code, MAX_LOT_CODE_BYTES)
        && valid_text(&record.actor, MAX_ACTOR_BYTES)
        && matches!(record.command_kind.as_str(), "bid" | "proxy" | "proxy_lower" | "notify")
        && record.command_kind.len() <= MAX_COMMAND_KIND_BYTES
        && record.amount >= 0
}

fn valid_text(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value == value.trim()
        && !value.chars().any(char::is_control)
}

fn valid_optional_text(value: &str, maximum: usize) -> bool {
    value.is_empty() || valid_text(value, maximum)
}

#[cfg(feature = "postgres")]
fn valid_decimal_id(value: &str, maximum: usize) -> bool {
    !value.is_empty() && value.len() <= maximum && value.bytes().all(|byte| byte.is_ascii_digit())
}

#[cfg(feature = "postgres")]
fn parse_world(value: &str) -> Result<SettlementWorld, ApplicationError> {
    super::sync::parse_world(value)
}

fn storage_error(message: &str) -> ApplicationError {
    ApplicationError::Repository(RepositoryError::Failed(message.to_owned()))
}

#[cfg(feature = "sqlite")]
fn capability_error() -> ApplicationError {
    ApplicationError::CapabilityUnavailable("论坛回帖导入仅支持 PostgreSQL 完整档")
}
