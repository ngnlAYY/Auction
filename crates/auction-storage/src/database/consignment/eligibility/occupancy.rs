//! 一口价登记额度与同件装备占用事实。

use auction_domain::UserId;
use auction_domain::consignment::{ListingEquipmentIdentity, ListingReason};
use auction_time::Timestamp;
use sqlx::{PgConnection, Postgres};
use uuid::Uuid;

use super::predicates::listing_sql;

/// 按卖家和数据库时刻查询滚动登记额度。
pub(crate) const HOURLY_COUNT_SQL: &str = "SELECT count(*) FROM public.consignment_listings l
     WHERE l.seller_user_id = $1 AND l.created_at > $2::timestamptz - interval '1 hour'
       AND l.created_at <= $2::timestamptz";

// 按名额度单向合并：匿名登记把同名本站账户（username 即 HV 角色名）的账户挂单一并计入，
// 防止账户额度用满后改走免登录入口绕过；账户登记的额度（HOURLY_COUNT_SQL、ACTIVE_COUNT_SQL）
// 不计入同名匿名挂单，否则任何人冒用他人角色名匿名登记就能占满真实账户的额度。

/// 按自填名（lower 等值）和数据库时刻查询匿名滚动登记额度，含同名账户的登记。
pub(crate) const ANONYMOUS_HOURLY_COUNT_SQL: &str = concat!(
    "SELECT count(*) FROM public.consignment_listings l WHERE ",
    listing_sql!(anonymous_name_quota, "$1"),
    " AND l.created_at > $2::timestamptz - interval '1 hour' AND l.created_at <= $2::timestamptz"
);

/// 按自填名（lower 等值）和数据库时刻查询匿名跨世界有效活动额度，含同名账户的活动挂单；
/// 账户挂单的有效性按其账户资格判定，与 [`ACTIVE_COUNT_SQL`] 同一口径。
pub(crate) const ANONYMOUS_ACTIVE_COUNT_SQL: &str = concat!(
    "SELECT count(*) FROM public.consignment_listings l",
    listing_sql!(eligibility_joins),
    "WHERE ",
    listing_sql!(anonymous_name_quota, "$1"),
    " AND ",
    listing_sql!(effective_active, "$2")
);

/// 全站尚在解析期内的匿名挂单数（全局上限）。
const ANONYMOUS_RESOLVING_COUNT_SQL: &str = "SELECT count(*) FROM public.consignment_listings l
     WHERE l.seller_user_id IS NULL AND l.state = 'resolving' AND l.expires_at > $1::timestamptz";

/// 按卖家和数据库时刻查询跨世界有效活动额度。
pub(crate) const ACTIVE_COUNT_SQL: &str = concat!(
    "SELECT count(*) FROM public.consignment_listings l",
    listing_sql!(eligibility_joins),
    "WHERE l.seller_user_id = $1 AND ",
    listing_sql!(effective_active, "$2")
);

/// 在卖家锁内，计数滚动一小时内已接受的登记，包含其后终结的挂单。
pub(crate) async fn hourly_registration_count(
    tx: &mut PgConnection,
    seller_user_id: UserId,
    as_of: Timestamp,
) -> Result<i64, sqlx::Error> {
    seller_count(tx, HOURLY_COUNT_SQL, seller_user_id, as_of).await
}

/// 在同一个卖家锁内，计数该卖家两个世界的有效活动挂单。
pub(crate) async fn effective_activity_count(
    tx: &mut PgConnection,
    seller_user_id: UserId,
    as_of: Timestamp,
) -> Result<i64, sqlx::Error> {
    seller_count(tx, ACTIVE_COUNT_SQL, seller_user_id, as_of).await
}

/// 在自填名会话锁及同名账户行锁内，按 lower 名称计数滚动一小时内已接受的登记
/// （匿名挂单与同名账户挂单）。
///
/// 不变量 2：匿名登记先经 [`super::accounts::lock_same_name_account`] 对同名账户行
/// `FOR UPDATE`，账户登记经 [`super::accounts::lock_seller_eligibility`] 锁同一行，两条路径在
/// 该行上串行；没有同名账户时由 Serializable 冲突重试裁定并发。
pub(crate) async fn anonymous_hourly_registration_count(
    tx: &mut PgConnection,
    seller_game_name: &str,
    as_of: Timestamp,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(ANONYMOUS_HOURLY_COUNT_SQL)
        .bind(seller_game_name)
        .bind(as_of)
        .fetch_one(tx)
        .await
}

/// 在自填名会话锁及同名账户行锁内，按 lower 名称计数两个世界的有效活动挂单
/// （匿名挂单与同名账户挂单），锁序同 [`anonymous_hourly_registration_count`]。
pub(crate) async fn anonymous_activity_count(
    tx: &mut PgConnection,
    seller_game_name: &str,
    as_of: Timestamp,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(ANONYMOUS_ACTIVE_COUNT_SQL)
        .bind(seller_game_name)
        .bind(as_of)
        .fetch_one(tx)
        .await
}

/// 全站匿名解析中的挂单数；超限的并发登记由 Serializable 冲突重试裁定。
pub(crate) async fn anonymous_resolving_count(
    tx: &mut PgConnection,
    as_of: Timestamp,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(ANONYMOUS_RESOLVING_COUNT_SQL).bind(as_of).fetch_one(tx).await
}

async fn seller_count(
    tx: &mut PgConnection,
    sql: &'static str,
    seller_user_id: UserId,
    as_of: Timestamp,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(sql).bind(seller_user_id.as_uuid()).bind(as_of).fetch_one(tx).await
}

/// 同件装备上的持久活动行及清退或占用事实。
#[derive(Debug)]
pub(crate) struct EquipmentOccupant {
    /// 调用方在此挂单上提交清退或继续判断。
    pub(crate) listing_id: Uuid,
    /// 用于判断本人跨模式重复；匿名挂单为空。
    pub(crate) seller_user_id: Option<UserId>,
    /// 匿名挂单自填名的 PostgreSQL lower 值，用于判断同名重复；账户挂单为空。
    pub(crate) seller_game_key: Option<String>,
    /// 活动且已发布时阻止其他挂单发布；仅在 forced_reason 为空时有效。
    pub(crate) published: bool,
    /// 非空时须先清退，为空时此行是有效活动。
    pub(crate) forced_reason: Option<ListingReason>,
}

/// 在命令、卖家、EID 会话锁及当前卖家行锁后，按 UUID 锁本件持久活动行并返回占用事实。
pub(crate) async fn lock_equipment_occupants(
    tx: &mut PgConnection,
    identity: &ListingEquipmentIdentity,
    as_of: Timestamp,
) -> Result<Vec<EquipmentOccupant>, sqlx::Error> {
    // 不变量 2、6：只清退持锁 EID 上已逻辑失效的行，未核验的其他卖家占用不阻止登记。
    let occupants = sqlx::query_as::<
        Postgres,
        (Uuid, Option<Uuid>, Option<String>, bool, Option<String>),
    >(concat!(
        "SELECT l.id, l.seller_user_id, lower(l.seller_game_name), ",
        listing_sql!(stored_published),
        ", ",
        listing_sql!(forced_reason, "$3"),
        " FROM public.consignment_listings l",
        listing_sql!(eligibility_joins),
        "WHERE l.world_id = $1 AND l.external_equipment_id = $2 AND ",
        listing_sql!(stored_active),
        " ORDER BY l.id FOR UPDATE OF l"
    ))
    .bind(identity.world_id())
    .bind(identity.external_equipment_id())
    .bind(as_of)
    .fetch_all(tx)
    .await?;
    occupants
        .into_iter()
        .map(|(listing_id, seller_user_id, seller_game_key, published, reason)| {
            let forced_reason = reason
                .as_deref()
                .map(ListingReason::parse)
                .transpose()
                .map_err(|error| sqlx::Error::Decode(Box::new(error)))?;
            Ok(EquipmentOccupant {
                listing_id,
                seller_user_id: seller_user_id.map(UserId::from_uuid),
                seller_game_key,
                published,
                forced_reason,
            })
        })
        .collect()
}
