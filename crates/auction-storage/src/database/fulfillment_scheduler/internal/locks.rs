//! 统一前缀与同 EID 的业务锁；外部观察在调用本模块之前已完成。

use super::sql_error;
use auction_application::{FundingError, SettlementFulfillmentClaim};
use sqlx::{Postgres, Transaction};

pub(super) async fn worlds(tx: &mut Transaction<'_, Postgres>) -> Result<(), FundingError> {
    // 与资金对账使用相同持久 code 排序；跨世界不拆事务。
    sqlx::query("SELECT id FROM public.worlds ORDER BY id FOR UPDATE")
        .fetch_all(&mut **tx)
        .await
        .map_err(sql_error)?;
    Ok(())
}

pub(super) async fn before_credentials(
    tx: &mut Transaction<'_, Postgres>,
    claim: &SettlementFulfillmentClaim,
) -> Result<(), FundingError> {
    // 同外部EID可能有多个本地equipment行；不能只锁claim中的UUID。
    for query in [
        "SELECT a.id FROM public.auctions a WHERE a.id=$1 OR a.id IN (SELECT o.auction_id FROM public.settlement_obligations o JOIN public.equipment e ON e.id=o.equipment_id WHERE e.external_equipment_id=$2 AND e.world_id=$3) ORDER BY a.id FOR UPDATE",
        "SELECT r.id FROM public.settlement_recoveries r WHERE r.auction_id=$1 OR r.lot_id IN (SELECT o.lot_id FROM public.settlement_obligations o JOIN public.equipment e ON e.id=o.equipment_id WHERE e.external_equipment_id=$2 AND e.world_id=$3) ORDER BY r.id FOR UPDATE",
        "SELECT l.id FROM public.lots l WHERE l.auction_id=$1 OR l.id IN (SELECT o.lot_id FROM public.settlement_obligations o JOIN public.equipment e ON e.id=o.equipment_id WHERE e.external_equipment_id=$2 AND e.world_id=$3) ORDER BY l.id FOR UPDATE",
    ] {
        sqlx::query(query)
            .bind(claim.auction_id.as_uuid())
            .bind(&claim.external_equipment_id)
            .bind(super::validation::world(claim.equipment_world))
            .fetch_all(&mut **tx)
            .await
            .map_err(sql_error)?;
    }
    sqlx::query("SELECT id FROM public.equipment WHERE id=$1 OR (external_equipment_id=$2 AND world_id=$3) ORDER BY id FOR UPDATE")
        .bind(claim.equipment_id.as_uuid()).bind(&claim.external_equipment_id)
        .bind(super::validation::world(claim.equipment_world)).fetch_all(&mut **tx).await.map_err(sql_error)?;
    Ok(())
}

pub(super) async fn after_credentials(
    tx: &mut Transaction<'_, Postgres>,
    claim: &SettlementFulfillmentClaim,
) -> Result<(), FundingError> {
    // users→obligations→actions→deliveries→fulfillments；直接依赖与同资产上下文均在同一序列。
    sqlx::query("SELECT id FROM public.users WHERE id=$1 FOR UPDATE")
        .bind(claim.recipient_user_id)
        .fetch_all(&mut **tx)
        .await
        .map_err(sql_error)?;
    sqlx::query("SELECT o.id FROM public.settlement_obligations o WHERE o.auction_id=$1 OR o.equipment_id IN (SELECT id FROM public.equipment WHERE external_equipment_id=$2 AND world_id=$3) ORDER BY o.id FOR UPDATE")
        .bind(claim.auction_id.as_uuid()).bind(&claim.external_equipment_id)
        .bind(super::validation::world(claim.equipment_world)).fetch_all(&mut **tx).await.map_err(sql_error)?;
    sqlx::query("SELECT a.id FROM public.settlement_recovery_actions a JOIN public.settlement_recoveries r ON r.id=a.recovery_id WHERE r.auction_id=$1 OR r.lot_id IN (SELECT o.lot_id FROM public.settlement_obligations o JOIN public.equipment e ON e.id=o.equipment_id WHERE e.external_equipment_id=$2 AND e.world_id=$3) ORDER BY a.id FOR UPDATE OF a")
        .bind(claim.auction_id.as_uuid()).bind(&claim.external_equipment_id)
        .bind(super::validation::world(claim.equipment_world)).fetch_all(&mut **tx).await.map_err(sql_error)?;
    // 全部delivery含孤儿，后续冲突筛查必须可见；world前缀串行化本lane写入。
    sqlx::query("SELECT id FROM public.deliveries ORDER BY id FOR UPDATE")
        .fetch_all(&mut **tx)
        .await
        .map_err(sql_error)?;
    sqlx::query("SELECT f.obligation_id FROM public.settlement_obligation_fulfillments f JOIN public.settlement_obligations o ON o.id=f.obligation_id WHERE o.auction_id=$1 OR o.equipment_id IN (SELECT id FROM public.equipment WHERE external_equipment_id=$2 AND world_id=$3) ORDER BY f.obligation_id FOR UPDATE OF f")
        .bind(claim.auction_id.as_uuid()).bind(&claim.external_equipment_id)
        .bind(super::validation::world(claim.equipment_world)).fetch_all(&mut **tx).await.map_err(sql_error)?;
    Ok(())
}
