//! 恢复记录落定 resolution（成交撤销）之后结算义务是否仍可执行的唯一谓词：履约领取、
//! prepare/activate/satisfy 的最终事务与 Admin 重新激活结算投递共用同一口径，按义务在恢复记录中
//! 的角色与 resolution 判定。

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
pub(crate) async fn recovery_allows_execution_now(
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

/// Admin 重新激活一条投递之前的结算资格判定结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettlementDeliveryEligibility {
    /// 投递不是结算来源（或投递不存在，由调用方按不存在处理），不受本谓词约束。
    NotSettlement,
    /// 来源义务仍可执行，且本投递就是义务履约的当前派发。
    Eligible,
    /// 成交已撤销使义务不再可执行，或本投递已不是义务的当前派发，或来源义务无法解析。
    Ineligible,
}

/// Admin 释放/重试/对账重试把投递改回 `pending` 之前，在同一事务内复核其结算义务资格。
///
/// 义务按投递自身的 `origin_kind='settlement'` 与 `origin_id` 解析，不经当前履约指针：指针换代后
/// 留下的旧派发也必须拦住，不能成为可重发的孤儿（不变量 4、6）。调用方已取得资金锁前缀、尚未锁
/// 投递行；这里按既有锁序先锁引用该义务的恢复记录（UUID 序），再锁义务，与恢复确认、结算履约同序，
/// 因此与并发确认要么在恢复行上串行、要么由 Serializable 冲突重试，不会出现确认之后仍复活投递的
/// 交错（不变量 2）。履约行只读，其行锁随后由调用方在投递行之后取得。
pub(crate) async fn lock_settlement_delivery_eligibility(
    transaction: &mut Transaction<'_, Postgres>,
    delivery_id: Uuid,
) -> Result<SettlementDeliveryEligibility, sqlx::Error> {
    let origin: Option<(String, String)> =
        sqlx::query_as("SELECT origin_kind, origin_id FROM public.deliveries WHERE id = $1")
            .bind(delivery_id)
            .fetch_optional(&mut **transaction)
            .await?;
    let Some((origin_kind, origin_id)) = origin else {
        return Ok(SettlementDeliveryEligibility::NotSettlement);
    };
    if origin_kind != "settlement" {
        return Ok(SettlementDeliveryEligibility::NotSettlement);
    }
    // 结算来源只写规范义务 UUID；无法解析即无法证明资格，失败关闭。
    let Some(obligation_id) = Uuid::parse_str(&origin_id)
        .ok()
        .filter(|value| !value.is_nil() && value.hyphenated().to_string() == origin_id)
    else {
        return Ok(SettlementDeliveryEligibility::Ineligible);
    };
    sqlx::query(
        r#"SELECT id
           FROM public.settlement_recoveries
           WHERE $1 IN (payment_obligation_id, equipment_obligation_id,
                        seller_payout_obligation_id, buyer_refund_obligation_id)
           ORDER BY id
           FOR UPDATE"#,
    )
    .bind(obligation_id)
    .execute(&mut **transaction)
    .await?;
    let obligation: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM public.settlement_obligations WHERE id = $1 FOR UPDATE")
            .bind(obligation_id)
            .fetch_optional(&mut **transaction)
            .await?;
    if obligation.is_none() || !recovery_allows_execution_now(transaction, obligation_id).await? {
        return Ok(SettlementDeliveryEligibility::Ineligible);
    }
    let current: Option<Option<Uuid>> = sqlx::query_scalar(
        "SELECT delivery_id FROM public.settlement_obligation_fulfillments WHERE obligation_id = $1",
    )
    .bind(obligation_id)
    .fetch_optional(&mut **transaction)
    .await?;
    // 履约行存在时，只有它当前指向的派发可以被重新激活；没有履约行的历史义务沿用旧判定。
    if current.is_some_and(|current| current != Some(delivery_id)) {
        return Ok(SettlementDeliveryEligibility::Ineligible);
    }
    Ok(SettlementDeliveryEligibility::Eligible)
}
