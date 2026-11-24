//! 登记基线与拍卖碰撞的集合读取和追加 SQL。

use sqlx::{PgConnection, Postgres};
use uuid::Uuid;

use super::predicates::listing_sql;

/// 依挂单身份从登记事务的同一快照冻结基线；调用方须先插入首版挂单。
#[allow(dead_code, reason = "由后续任务 2D 调用")]
pub(crate) async fn insert_registration_baselines(
    tx: &mut PgConnection,
    listing_id: Uuid,
) -> Result<u64, sqlx::Error> {
    // 不变量 2、3、8：基线是实际 lot 集合，在登记顶层事务一次性追加。
    let result = sqlx::query(concat!(
        "INSERT INTO public.consignment_listing_lot_baselines (listing_id, lot_id)
         SELECT l.id, a.id FROM public.consignment_listings l
         JOIN public.equipment e ON ",
        listing_sql!(equipment_alias, "l.world_id", "l.external_equipment_id"),
        " JOIN public.lots a ON a.equipment_id = e.id WHERE l.id = $1"
    ))
    .bind(listing_id)
    .execute(tx)
    .await?;
    Ok(result.rows_affected())
}

/// 发布或写侧复核指定挂单的碰撞；不存在时返回 None。
#[allow(dead_code, reason = "由后续任务 2D 调用")]
pub(crate) async fn listing_collision(
    tx: &mut PgConnection,
    listing_id: Uuid,
) -> Result<Option<bool>, sqlx::Error> {
    sqlx::query_scalar(LISTING_COLLISION_SQL).bind(listing_id).fetch_optional(tx).await
}

/// 指定挂单的碰撞查询，与执行计划夹具共用。
pub(crate) const LISTING_COLLISION_SQL: &str = concat!(
    "SELECT ",
    listing_sql!(collision),
    " FROM public.consignment_listings l WHERE l.id = $1"
);

/// 读出首次碰撞关闭时尚未关联的全部 lot。同一锁内的写入可直接使用集合 INSERT。
#[allow(dead_code, reason = "由后续任务 2D / 2R 调用")]
pub(crate) async fn unassociated_collision_lots(
    tx: &mut PgConnection,
    listing_id: Uuid,
) -> Result<Vec<Uuid>, sqlx::Error> {
    sqlx::query_scalar::<Postgres, Uuid>(concat!(
        "SELECT a.id FROM public.consignment_listings l
         CROSS JOIN LATERAL (SELECT a.id ",
        listing_sql!(unassociated_lots),
        ") a
         WHERE l.id = $1 ORDER BY a.id"
    ))
    .bind(listing_id)
    .fetch_all(tx)
    .await
}

/// 复核已观察的指定 lot 是否适合追加；允许已终结挂单，不枚举后来出现的其他 lot。
#[allow(dead_code, reason = "由后续任务 2D / 2R 调用")]
pub(crate) async fn observed_collision_is_unassociated(
    tx: &mut PgConnection,
    listing_id: Uuid,
    observed_lot_id: Uuid,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(concat!(
        "SELECT EXISTS (
             SELECT 1 FROM public.consignment_listings l WHERE l.id = $1
             AND EXISTS (SELECT 1 ",
        listing_sql!(unassociated_lots),
        " AND a.id = $2)
         )"
    ))
    .bind(listing_id)
    .bind(observed_lot_id)
    .fetch_one(tx)
    .await
}

/// 首次关闭时一次追加全部差集。参数依次为挂单 UUID、数据库时刻、命令 UUID。
#[allow(dead_code, reason = "由后续任务 2D 调用")]
pub(crate) const INSERT_COLLISION_SET_SQL: &str = concat!(
    // 不变量 2、3、8：由调用方持目标锁并和首次关闭事件同提交。
    "INSERT INTO public.consignment_listing_auction_conflicts
         (listing_id, lot_id, detected_at, command_id)
     SELECT l.id, a.id, $2, $3 FROM public.consignment_listings l
     CROSS JOIN LATERAL (SELECT a.id ",
    listing_sql!(unassociated_lots),
    ") a
     WHERE l.id = $1
     ON CONFLICT (listing_id, lot_id) DO NOTHING"
);

/// 追加已观察的指定碰撞。前三个参数同集合插入，第四个为该 lot UUID。
#[allow(dead_code, reason = "由后续任务 2D / 2R 调用")]
pub(crate) const INSERT_OBSERVED_COLLISION_SQL: &str = concat!(
    // 不变量 2、8：终态之后只追加此前实际观察过的具体 lot，由调用方持目标锁。
    "INSERT INTO public.consignment_listing_auction_conflicts
         (listing_id, lot_id, detected_at, command_id)
     SELECT l.id, a.id, $2, $3 FROM public.consignment_listings l
     CROSS JOIN LATERAL (SELECT a.id ",
    listing_sql!(unassociated_lots),
    " AND a.id = $4) a
     WHERE l.id = $1
     ON CONFLICT (listing_id, lot_id) DO NOTHING"
);
