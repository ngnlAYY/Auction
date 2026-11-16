//! 恢复记录落定 resolution（成交撤销）之后结算义务是否仍可执行的唯一谓词：履约领取与
//! prepare/activate/satisfy 的最终事务共用同一口径，按义务在恢复记录中的角色与 resolution 判定。

use sqlx::Postgres;
use uuid::Uuid;

use crate::database::TrackedTransaction as Transaction;

/// `o` 为 `public.settlement_obligations` 别名时义务仍可执行的 SQL 谓词。成交撤销后，恢复记录的
/// 付款、交付与卖方货款义务都不再执行（装备已收回、重列或寄回卖家，绝不能再寄出或付款）；买方
/// 退款只在 `equipment_recovered` 之后仍可执行，那条路径正是向已付款的买方退款。不属于任何恢复
/// 记录的义务（流拍/撤拍寄回等）不受影响。
macro_rules! recovery_allows_execution {
    () => {
        r#"NOT EXISTS (
               SELECT 1
               FROM public.settlement_recoveries AS resolved
               WHERE resolved.resolution IS NOT NULL
                 AND (resolved.payment_obligation_id = o.id
                      OR resolved.equipment_obligation_id = o.id
                      OR resolved.seller_payout_obligation_id = o.id
                      OR (resolved.buyer_refund_obligation_id = o.id
                          AND resolved.resolution <> 'equipment_recovered')))"#
    };
}
pub(super) use recovery_allows_execution;

/// 最终事务内复核资格；调用方已按既有锁序锁定恢复记录与义务，不满足的义务失败关闭，
/// 领取过滤与这里的复核共同保证撤销后的旧 claim 不能继续推进。
pub(super) async fn recovery_allows_execution_now(
    transaction: &mut Transaction<'_, Postgres>,
    obligation_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let allowed: Option<bool> = sqlx::query_scalar(concat!(
        "SELECT ",
        recovery_allows_execution!(),
        " FROM public.settlement_obligations AS o WHERE o.id = $1"
    ))
    .bind(obligation_id)
    .fetch_optional(&mut **transaction)
    .await?;
    Ok(allowed.unwrap_or(false))
}
