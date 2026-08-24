//! PostgreSQL 论坛主题同步存储。
//!
//! `auction_forum_sync` 是远程主题投影的唯一 owner。领取、快照读取、发布确认和失败退避都
//! 使用持久化 token/owner/fencing；SQLite Lite 不提供该外部投影能力，明确返回能力缺失。

#[cfg(feature = "postgres")]
use auction_application::ForumLotSnapshot;
#[cfg(feature = "postgres")]
use auction_application::forum_world_name;
use auction_application::{
    ApplicationError, ApplicationFuture, ForumAuctionSnapshot, ForumPublishedThread,
    ForumRenderedThread, ForumSyncClaim, ForumSyncStore, RepositoryError,
};
#[cfg(feature = "postgres")]
use auction_domain::{AuctionId, SettlementWorld};
use chrono::{DateTime, Utc};
#[cfg(feature = "postgres")]
use uuid::Uuid;

use super::super::DatabaseStore;

const MAX_CLAIM_BATCH_SIZE: u16 = 100;
const MAX_WORKER_ID_BYTES: usize = 256;
const MAX_FAILURE_CODE_BYTES: usize = 128;
const MAX_FAILURE_DETAIL_BYTES: usize = 2048;
const MAX_THREAD_ID_BYTES: usize = 64;
const MAX_THREAD_URL_BYTES: usize = 2048;
const MAX_RENDERED_TITLE_BYTES: usize = 512;
const MAX_RENDERED_BODY_BYTES: usize = 65_531;

impl ForumSyncStore for DatabaseStore {
    fn claim_due(
        &self,
        worker_id: String,
        now: DateTime<Utc>,
        lease_until: DateTime<Utc>,
        limit: u16,
    ) -> ApplicationFuture<'_, Vec<ForumSyncClaim>> {
        Box::pin(async move {
            if !valid_text(&worker_id, MAX_WORKER_ID_BYTES)
                || now.timestamp_nanos_opt().is_none()
                || lease_until <= now
                || limit == 0
                || limit > MAX_CLAIM_BATCH_SIZE
            {
                return Err(storage_error("论坛同步领取参数无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    claim_due_postgres(store, &worker_id, now, lease_until, limit).await
                }
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(_) => Err(capability_error()),
            }
        })
    }

    fn load_snapshot(&self, claim: ForumSyncClaim) -> ApplicationFuture<'_, ForumAuctionSnapshot> {
        Box::pin(async move {
            if !valid_claim(&claim) {
                return Err(storage_error("论坛同步快照 claim 无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => load_snapshot_postgres(store, &claim).await,
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(_) => Err(capability_error()),
            }
        })
    }

    fn publish(
        &self,
        claim: ForumSyncClaim,
        rendered: ForumRenderedThread,
        published: ForumPublishedThread,
        published_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            if !valid_claim(&claim)
                || !valid_rendered(&rendered)
                || !valid_published(&published)
                || published_at.timestamp_nanos_opt().is_none()
            {
                return Err(storage_error("论坛同步发布确认参数无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    publish_postgres(store, &claim, &rendered, &published, published_at).await
                }
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(_) => Err(capability_error()),
            }
        })
    }

    fn fail(
        &self,
        claim: ForumSyncClaim,
        retry_at: DateTime<Utc>,
        failure_code: String,
        safe_detail: String,
    ) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            if !valid_claim(&claim)
                || retry_at.timestamp_nanos_opt().is_none()
                || !valid_text(&failure_code, MAX_FAILURE_CODE_BYTES)
                || !valid_text(&safe_detail, MAX_FAILURE_DETAIL_BYTES)
            {
                return Err(storage_error("论坛同步失败记录参数无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    fail_postgres(store, &claim, retry_at, &failure_code, &safe_detail).await
                }
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(_) => Err(capability_error()),
            }
        })
    }
}

#[cfg(feature = "postgres")]
async fn claim_due_postgres(
    store: &super::super::PostgresStore,
    worker_id: &str,
    now: DateTime<Utc>,
    lease_until: DateTime<Utc>,
    limit: u16,
) -> Result<Vec<ForumSyncClaim>, ApplicationError> {
    use sqlx::Row;

    let mut transaction =
        store.pool().begin().await.map_err(|_| storage_error("开启论坛同步领取事务失败"))?;
    sqlx::query(
        "INSERT INTO public.auction_forum_sync (auction_id, source_revision)
         SELECT a.id, a.state_version
         FROM public.auctions AS a
         WHERE a.status IN ('preview', 'live', 'final', 'settling', 'settled', 'cancelled')
         ON CONFLICT (auction_id) DO NOTHING",
    )
    .execute(&mut *transaction)
    .await
    .map_err(|_| storage_error("初始化论坛同步投影失败"))?;

    let rows = sqlx::query(
        "SELECT sync.auction_id, auction.state_version
         FROM public.auction_forum_sync AS sync
         JOIN public.auctions AS auction ON auction.id = sync.auction_id
         WHERE auction.status IN ('preview', 'live', 'final', 'settling', 'settled', 'cancelled')
           AND (
               (sync.status IN ('pending', 'failed')
                AND (sync.last_error_code IS NULL OR sync.last_error_code NOT IN (
                    'invalid_snapshot', 'body_too_large', 'configuration',
                    'credential_unavailable', 'protocol_drift', 'write_unknown'
                ))
                AND sync.next_attempt_at <= $1)
               OR (sync.status = 'published' AND sync.source_revision < auction.state_version)
               OR (sync.status = 'publishing'
                   AND (sync.lease_expires_at IS NULL OR sync.lease_expires_at <= $1))
           )
         ORDER BY sync.next_attempt_at, sync.auction_id
         LIMIT $2
         FOR UPDATE OF sync SKIP LOCKED",
    )
    .bind(now)
    .bind(i64::from(limit))
    .fetch_all(&mut *transaction)
    .await
    .map_err(|_| storage_error("读取 due 论坛同步任务失败"))?;

    let mut claims = Vec::with_capacity(rows.len());
    for row in rows {
        let auction_id: Uuid =
            row.try_get("auction_id").map_err(|_| storage_error("读取论坛同步拍卖标识失败"))?;
        let source_revision: i64 =
            row.try_get("state_version").map_err(|_| storage_error("读取论坛同步源版本失败"))?;
        if source_revision <= 0 {
            return Err(storage_error("论坛同步源版本无效"));
        }
        let claim_token = Uuid::new_v4();
        let updated = sqlx::query(
            "UPDATE public.auction_forum_sync
             SET status = 'publishing',
                 claim_token = $2,
                 claim_owner = $3,
                 claim_fencing = claim_fencing + 1,
                 lease_expires_at = $4,
                 source_revision = $5,
                 attempt_count = attempt_count + 1,
                 last_error_code = NULL,
                 last_error_detail = NULL,
                 updated_at = $6
             WHERE auction_id = $1
               AND (claim_token IS NULL OR lease_expires_at IS NULL OR lease_expires_at <= $6)
             RETURNING claim_fencing",
        )
        .bind(auction_id)
        .bind(claim_token)
        .bind(worker_id)
        .bind(lease_until)
        .bind(source_revision)
        .bind(now)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|_| storage_error("写入论坛同步租约失败"))?;
        let Some(updated) = updated else {
            return Err(storage_error("论坛同步租约在事务内发生变化"));
        };
        let claim_fencing: i64 = updated
            .try_get("claim_fencing")
            .map_err(|_| storage_error("读取论坛同步 fencing 失败"))?;
        if claim_fencing < 1 {
            return Err(storage_error("论坛同步 fencing 无效"));
        }
        claims.push(ForumSyncClaim {
            auction_id: AuctionId::from_uuid(auction_id),
            claim_token,
            claim_fencing,
            worker_id: worker_id.to_owned(),
            claimed_at: now,
            lease_until,
            source_revision,
        });
    }
    transaction.commit().await.map_err(|_| storage_error("提交论坛同步领取事务失败"))?;
    Ok(claims)
}

#[cfg(feature = "postgres")]
async fn load_snapshot_postgres(
    store: &super::super::PostgresStore,
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
        "SELECT l.lot_number, l.status, l.start_price, l.current_price,
                l.effective_end_at, e.name AS equipment_name, e.category AS equipment_category,
                submission.seller AS seller_name, leader.username AS leader_name
         FROM public.lots AS l
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

#[cfg(feature = "postgres")]
fn public_auction_path(world: SettlementWorld, auction_id: uuid::Uuid) -> String {
    let world_name = forum_world_name(world);
    if world_name == "main" {
        format!("/?auction={auction_id}")
    } else {
        format!("/?auction={auction_id}&world={world_name}")
    }
}

#[cfg(feature = "postgres")]
async fn publish_postgres(
    store: &super::super::PostgresStore,
    claim: &ForumSyncClaim,
    rendered: &ForumRenderedThread,
    published: &ForumPublishedThread,
    published_at: DateTime<Utc>,
) -> Result<(), ApplicationError> {
    let result = sqlx::query(
        "UPDATE public.auction_forum_sync
         SET status = 'published',
             lease_expires_at = NULL,
             claim_token = NULL,
             claim_owner = NULL,
             title = $2,
             body = $3,
             thread_id = $4,
             thread_url = $5,
             last_error_code = NULL,
             last_error_detail = NULL,
             updated_at = $6
         WHERE auction_id = $1
           AND status = 'publishing'
           AND claim_token = $7
           AND claim_owner = $8
           AND claim_fencing = $9
           AND source_revision = $10
           AND lease_expires_at > $6",
    )
    .bind(claim.auction_id.as_uuid())
    .bind(&rendered.title)
    .bind(&rendered.body)
    .bind(&published.thread_id)
    .bind(&published.thread_url)
    .bind(published_at)
    .bind(claim.claim_token)
    .bind(&claim.worker_id)
    .bind(claim.claim_fencing)
    .bind(claim.source_revision)
    .execute(store.pool())
    .await
    .map_err(|_| storage_error("写入论坛同步发布证明失败"))?;
    if result.rows_affected() != 1 {
        return Err(storage_error("论坛同步发布 claim 已失效"));
    }
    Ok(())
}

#[cfg(feature = "postgres")]
async fn fail_postgres(
    store: &super::super::PostgresStore,
    claim: &ForumSyncClaim,
    retry_at: DateTime<Utc>,
    failure_code: &str,
    safe_detail: &str,
) -> Result<(), ApplicationError> {
    let result = sqlx::query(
        "UPDATE public.auction_forum_sync
         SET status = 'failed',
             next_attempt_at = $2,
             lease_expires_at = NULL,
             claim_token = NULL,
             claim_owner = NULL,
             last_error_code = $3,
             last_error_detail = $4,
             updated_at = CURRENT_TIMESTAMP
         WHERE auction_id = $1
           AND status = 'publishing'
           AND claim_token = $5
           AND claim_owner = $6
           AND claim_fencing = $7
           AND source_revision = $8
           AND lease_expires_at > CURRENT_TIMESTAMP",
    )
    .bind(claim.auction_id.as_uuid())
    .bind(retry_at)
    .bind(failure_code)
    .bind(safe_detail)
    .bind(claim.claim_token)
    .bind(&claim.worker_id)
    .bind(claim.claim_fencing)
    .bind(claim.source_revision)
    .execute(store.pool())
    .await
    .map_err(|_| storage_error("写入论坛同步失败退避状态失败"))?;
    if result.rows_affected() != 1 {
        return Err(storage_error("论坛同步失败 claim 已失效"));
    }
    Ok(())
}

fn valid_claim(claim: &ForumSyncClaim) -> bool {
    !claim.auction_id.is_nil()
        && !claim.claim_token.is_nil()
        && valid_text(&claim.worker_id, MAX_WORKER_ID_BYTES)
        && claim.claim_fencing > 0
        && claim.source_revision > 0
        && claim.claimed_at.timestamp_nanos_opt().is_some()
        && claim.lease_until > claim.claimed_at
}

fn valid_rendered(rendered: &ForumRenderedThread) -> bool {
    !rendered.title.is_empty()
        && rendered.title == rendered.title.trim()
        && rendered.title.len() <= MAX_RENDERED_TITLE_BYTES
        && !rendered.title.chars().any(char::is_control)
        && !rendered.body.trim().is_empty()
        && rendered.body.len() <= MAX_RENDERED_BODY_BYTES
        && !rendered
            .body
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
}

fn valid_published(published: &ForumPublishedThread) -> bool {
    published.board_id > 0
        && valid_decimal_id(&published.thread_id, MAX_THREAD_ID_BYTES)
        && valid_text(&published.thread_url, MAX_THREAD_URL_BYTES)
        && published.thread_url.starts_with("https://")
}

fn valid_text(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value == value.trim()
        && !value.chars().any(char::is_control)
}

fn valid_decimal_id(value: &str, maximum: usize) -> bool {
    !value.is_empty() && value.len() <= maximum && value.bytes().all(|byte| byte.is_ascii_digit())
}

#[cfg(feature = "postgres")]
pub(super) fn parse_world(value: &str) -> Result<SettlementWorld, ApplicationError> {
    match value {
        "main" => Ok(SettlementWorld::Main),
        "isekai" => Ok(SettlementWorld::Isekai),
        _ => Err(storage_error("论坛快照世界不在闭集内")),
    }
}

#[cfg(feature = "postgres")]
pub(super) fn lot_code(category: &str, lot_number: i32) -> Result<String, ApplicationError> {
    let prefix = match category {
        "One-Handed" => "One",
        "Two-Handed" => "Two",
        "Staff" => "Sta",
        "Shield" => "Shd",
        "Cloth" => "Clo",
        "Light" => "Lig",
        "Heavy" => "Hea",
        _ => return Err(storage_error("论坛快照装备分类不在闭集内")),
    };
    if !(1..=99).contains(&lot_number) {
        return Err(storage_error("论坛快照 lot 序号超出论坛编码范围"));
    }
    Ok(format!("{prefix}{lot_number:02}"))
}

fn storage_error(message: &str) -> ApplicationError {
    ApplicationError::Repository(RepositoryError::Failed(message.to_owned()))
}

#[cfg(feature = "sqlite")]
fn capability_error() -> ApplicationError {
    ApplicationError::CapabilityUnavailable("论坛同步仅支持 PostgreSQL 完整档")
}
