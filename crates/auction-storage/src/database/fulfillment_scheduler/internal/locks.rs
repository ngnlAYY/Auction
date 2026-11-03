//! 同 EID 的业务锁；world 锁前缀由 `settlement_funding::lock_funding_worlds` 统一拥有，
//! 外部观察在调用本模块之前已完成。
//! 仅加锁的查询完整执行但不收集行；需要检查数量的 delivery 冲突探针仍保留有界结果。

use crate::database::TrackedTransaction as Transaction;
use std::sync::LazyLock;

use super::sql_error;
use auction_application::{FundingError, SettlementFulfillmentClaim};
use sqlx::Postgres;

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
            .bind(claim.equipment_world.as_str())
            .execute(&mut **tx)
            .await
            .map_err(sql_error)?;
    }
    sqlx::query("SELECT id FROM public.equipment WHERE id=$1 OR (external_equipment_id=$2 AND world_id=$3) ORDER BY id FOR UPDATE")
        .bind(claim.equipment_id.as_uuid()).bind(&claim.external_equipment_id)
        .bind(claim.equipment_world.as_str()).execute(&mut **tx).await.map_err(sql_error)?;
    Ok(())
}

pub(super) async fn after_credentials(
    tx: &mut Transaction<'_, Postgres>,
    claim: &SettlementFulfillmentClaim,
) -> Result<(), FundingError> {
    // users→obligations→actions→deliveries→fulfillments；直接依赖与同资产上下文均在同一序列。
    sqlx::query("SELECT id FROM public.users WHERE id=$1 FOR UPDATE")
        .bind(claim.recipient_user_id)
        .execute(&mut **tx)
        .await
        .map_err(sql_error)?;
    sqlx::query("SELECT o.id FROM public.settlement_obligations o WHERE o.auction_id=$1 OR o.equipment_id IN (SELECT id FROM public.equipment WHERE external_equipment_id=$2 AND world_id=$3) ORDER BY o.id FOR UPDATE")
        .bind(claim.auction_id.as_uuid()).bind(&claim.external_equipment_id)
        .bind(claim.equipment_world.as_str()).execute(&mut **tx).await.map_err(sql_error)?;
    sqlx::query("SELECT a.id FROM public.settlement_recovery_actions a JOIN public.settlement_recoveries r ON r.id=a.recovery_id WHERE r.auction_id=$1 OR r.lot_id IN (SELECT o.lot_id FROM public.settlement_obligations o JOIN public.equipment e ON e.id=o.equipment_id WHERE e.external_equipment_id=$2 AND e.world_id=$3) ORDER BY a.id FOR UPDATE OF a")
        .bind(claim.auction_id.as_uuid()).bind(&claim.external_equipment_id)
        .bind(claim.equipment_world.as_str()).execute(&mut **tx).await.map_err(sql_error)?;
    // 只锁当前投递及同 EID/world 的显式资产引用/owner 关联，历史通知不占行锁。
    // 上限包含一条溢出探针；超过上限整事务拒绝，绝不截断后声称已排除冲突。
    // 未知 schema/孤儿仍由 conflicts 的完整谓词读失败关闭；Serializable 的 SIREAD
    // 保留这些负向证明的幻行保护，world 前缀与同资产业务锁继续串行化写入。
    // 只缓存固定 SQL 文本，每次仍重新取得同一有序锁集合。
    static QUERY: LazyLock<String> = LazyLock::new(|| {
        "SELECT d.id FROM public.deliveries d
         WHERE d.id=$1 OR (
           NOT EXISTS(SELECT 1 FROM public.settlement_obligation_fulfillments f
             WHERE f.delivery_id=d.id AND f.execution_state='satisfied' AND f.proof_digest IS NOT NULL)
           AND NOT EXISTS(SELECT 1 FROM public.settlement_recovery_actions a
             WHERE a.delivery_id=d.id AND a.execution_state='satisfied' AND a.proof_digest IS NOT NULL)
           AND NOT EXISTS(SELECT 1 FROM public.settlement_legacy_self_reconciliations r
             WHERE /* trusted_legacy_self */)
           AND (EXISTS(SELECT 1 FROM public.settlement_obligations o
             JOIN public.equipment e ON e.id=o.equipment_id
             WHERE d.origin_kind='settlement' AND d.origin_id=o.id::text
               AND e.external_equipment_id=$2 AND e.world_id=$3)
           OR EXISTS(SELECT 1 FROM public.settlement_recovery_actions a
             JOIN public.settlement_recoveries r ON r.id=a.recovery_id
             JOIN public.settlement_obligations o ON o.id=r.payment_obligation_id
             JOIN public.equipment e ON e.id=o.equipment_id
             WHERE d.origin_id=a.id::text AND e.external_equipment_id=$2 AND e.world_id=$3)
           OR (d.payload->>'world'=$3 AND (
             d.payload->'attachment_ids' @> jsonb_build_array($2::text)
             OR EXISTS(SELECT 1 FROM public.equipment e
               WHERE e.external_equipment_id=$2 AND e.world_id=$3
                 AND d.payload->'equipment_secret_refs'
                     @> jsonb_build_array(jsonb_build_object('id',e.id::text)))))))
         ORDER BY d.id LIMIT 257 FOR UPDATE OF d"
        .replace("/* trusted_legacy_self */",
            crate::database::legacy_self_proof::TRUSTED_DELIVERY_PREDICATE)
    });
    let deliveries = sqlx::query_scalar::<_, uuid::Uuid>(sqlx::AssertSqlSafe(QUERY.as_str()))
        .bind(claim.delivery_id)
        .bind(&claim.external_equipment_id)
        .bind(claim.equipment_world.as_str())
        .fetch_all(&mut **tx)
        .await
        .map_err(sql_error)?;
    if deliveries.len() > 256 {
        return Err(FundingError::Unproven);
    }
    sqlx::query("SELECT f.obligation_id FROM public.settlement_obligation_fulfillments f JOIN public.settlement_obligations o ON o.id=f.obligation_id WHERE o.auction_id=$1 OR o.equipment_id IN (SELECT id FROM public.equipment WHERE external_equipment_id=$2 AND world_id=$3) ORDER BY f.obligation_id FOR UPDATE OF f")
        .bind(claim.auction_id.as_uuid()).bind(&claim.external_equipment_id)
        .bind(claim.equipment_world.as_str()).execute(&mut **tx).await.map_err(sql_error)?;
    Ok(())
}
