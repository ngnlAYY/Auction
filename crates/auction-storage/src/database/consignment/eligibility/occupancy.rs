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

/// 按卖家和数据库时刻查询跨世界有效活动额度。
pub(crate) const ACTIVE_COUNT_SQL: &str = concat!(
    "SELECT count(*) FROM public.consignment_listings l",
    listing_sql!(eligibility_joins),
    "WHERE l.seller_user_id = $1 AND ",
    listing_sql!(effective_active, "$2")
);

/// 在卖家锁内，计数滚动一小时内已接受的登记，包含其后终结的挂单。
#[allow(dead_code, reason = "由后续任务 2D 调用")]
pub(crate) async fn hourly_registration_count(
    tx: &mut PgConnection,
    seller_user_id: UserId,
    as_of: Timestamp,
) -> Result<i64, sqlx::Error> {
    seller_count(tx, HOURLY_COUNT_SQL, seller_user_id, as_of).await
}

/// 在同一个卖家锁内，计数该卖家两个世界的有效活动挂单。
#[allow(dead_code, reason = "由后续任务 2D 调用")]
pub(crate) async fn effective_activity_count(
    tx: &mut PgConnection,
    seller_user_id: UserId,
    as_of: Timestamp,
) -> Result<i64, sqlx::Error> {
    seller_count(tx, ACTIVE_COUNT_SQL, seller_user_id, as_of).await
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
#[allow(dead_code, reason = "由后续任务 2D 调用")]
pub(crate) struct EquipmentOccupant {
    /// 调用方在此挂单上提交清退或继续判断。
    pub(crate) listing_id: Uuid,
    /// 用于判断本人跨模式重复。
    pub(crate) seller_user_id: UserId,
    /// 活动且已发布时阻止其他挂单发布；仅在 forced_reason 为空时有效。
    pub(crate) published: bool,
    /// 非空时须先清退，为空时此行是有效活动。
    pub(crate) forced_reason: Option<ListingReason>,
}

/// 在命令、卖家、EID 会话锁及当前卖家行锁后，按 UUID 锁本件持久活动行并返回占用事实。
#[allow(dead_code, reason = "由后续任务 2D 调用")]
pub(crate) async fn lock_equipment_occupants(
    tx: &mut PgConnection,
    identity: &ListingEquipmentIdentity,
    as_of: Timestamp,
) -> Result<Vec<EquipmentOccupant>, sqlx::Error> {
    // 不变量 2、6：只清退持锁 EID 上已逻辑失效的行，未核验的其他卖家占用不阻止登记。
    let occupants = sqlx::query_as::<Postgres, (Uuid, Uuid, bool, Option<String>)>(concat!(
        "SELECT l.id, l.seller_user_id, ",
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
        .map(|(listing_id, seller_user_id, published, reason)| {
            let forced_reason = reason
                .as_deref()
                .map(ListingReason::parse)
                .transpose()
                .map_err(|error| sqlx::Error::Decode(Box::new(error)))?;
            Ok(EquipmentOccupant {
                listing_id,
                seller_user_id: UserId::from_uuid(seller_user_id),
                published,
                forced_reason,
            })
        })
        .collect()
}
