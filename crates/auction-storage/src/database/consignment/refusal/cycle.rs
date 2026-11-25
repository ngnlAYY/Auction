//! 买家拒收周期：按发生时刻归属周期，解禁路径原子关闭旧周期、开启新周期。
//!
//! 保护决定 23 与不变量 2、8：已用于上次处罚的拒收不再凑数；迟到证据按发生时周期，只能得到
//! 原效果或 `cycle_closed`；周期只前进不回写，旧周期的待执行处罚转 `cycle_closed`，不编辑历史。

use auction_domain::UserId;
use auction_domain::consignment::refusal::cycle_id;
use auction_time::Timestamp;
use sqlx::PgConnection;
use uuid::Uuid;

use super::CycleTransition;
use super::account_work::cancel_open_work;
use super::buyer_lock::ensure_buyer_lock_held;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::deposit::primitives::database_now;

/// 解禁关闭周期时记录的原因（`close_reason`，至多 64 字符）。
const CLOSE_REASON_UNBLOCKED: &str = "account_unblocked";

/// 事实所属周期；是否已关闭由提交入口在需要时另行读取。
pub(super) struct OccurrenceCycle {
    pub(super) cycle_id: Uuid,
}

/// 按事实发生时刻取买家的归属周期；买家还没有任何周期时以 `now` 开启第 1 个。
///
/// 归属规则：`opened_at <= returned_at` 中开启最晚者；早于全部周期开启时刻的事实归最早周期
/// （首个周期按需创建，开启时刻晚于它此前已发生的退回）。调用方必须已持有买家锁。
pub(super) async fn cycle_for_occurrence(
    conn: &mut PgConnection,
    buyer: UserId,
    returned_at: Timestamp,
    now: Timestamp,
) -> Result<OccurrenceCycle, WriteError> {
    let at_or_before: Option<(Uuid, String)> = sqlx::query_as(
        "SELECT id, state FROM public.consignment_refusal_cycles
         WHERE buyer_user_id = $1 AND opened_at <= $2
         ORDER BY opened_at DESC, ordinal DESC LIMIT 1",
    )
    .bind(buyer.as_uuid())
    .bind(returned_at)
    .fetch_optional(&mut *conn)
    .await?;
    let found = match at_or_before {
        Some(found) => Some(found),
        None => {
            sqlx::query_as(
                "SELECT id, state FROM public.consignment_refusal_cycles
                 WHERE buyer_user_id = $1 ORDER BY ordinal ASC LIMIT 1",
            )
            .bind(buyer.as_uuid())
            .fetch_optional(&mut *conn)
            .await?
        }
    };
    if let Some((cycle_id, _state)) = found {
        return Ok(OccurrenceCycle { cycle_id });
    }
    let opened = open_cycle(conn, buyer, now).await?;
    Ok(OccurrenceCycle { cycle_id: opened })
}

/// 开启买家的下一个周期（序号为已有最大序号加一）；周期 ID 由买家与序号确定性派生。
async fn open_cycle(
    conn: &mut PgConnection,
    buyer: UserId,
    now: Timestamp,
) -> Result<Uuid, WriteError> {
    let last: Option<i64> = sqlx::query_scalar(
        "SELECT max(ordinal) FROM public.consignment_refusal_cycles WHERE buyer_user_id = $1",
    )
    .bind(buyer.as_uuid())
    .fetch_one(&mut *conn)
    .await?;
    let ordinal = last
        .unwrap_or(0)
        .checked_add(1)
        .ok_or_else(|| WriteError::invariant("consignment_refusal_cycle_ordinal"))?;
    let id = cycle_id(buyer, u64::try_from(ordinal)?);
    sqlx::query(
        "INSERT INTO public.consignment_refusal_cycles (id, buyer_user_id, ordinal, state, opened_at)
         VALUES ($1, $2, $3, 'open', $4)",
    )
    .bind(id)
    .bind(buyer.as_uuid())
    .bind(ordinal)
    .bind(now)
    .execute(&mut *conn)
    .await?;
    Ok(id)
}

/// 解禁路径：原子关闭买家的 open 周期并开启下一个周期。
///
/// 旧周期里仍 pending 的处罚转 `cycle_closed`（新购阻断随之解除），其待执行账号工作取消；已
/// `applied` 的处罚与已完成的工作保持原状（只追加历史）。买家没有 open 周期时只开启新周期。
/// 调用方（解禁入口）必须持有买家用户锁并在同一事务内调用，使“解禁”与“周期边界”同提交。
pub(crate) async fn close_cycle_and_open_next(
    conn: &mut PgConnection,
    buyer: UserId,
) -> Result<CycleTransition, WriteError> {
    ensure_buyer_lock_held(conn, buyer).await?;
    let now = database_now(conn).await?;
    let open: Option<(Uuid, i64)> = sqlx::query_as(
        "SELECT id, ordinal FROM public.consignment_refusal_cycles
         WHERE buyer_user_id = $1 AND state = 'open' FOR UPDATE",
    )
    .bind(buyer.as_uuid())
    .fetch_optional(&mut *conn)
    .await?;
    let Some((closed_cycle_id, _ordinal)) = open else {
        let opened_cycle_id = open_cycle(conn, buyer, now).await?;
        return Ok(CycleTransition {
            closed_cycle_id: None,
            opened_cycle_id,
            closed_sanction_id: None,
        });
    };
    // 先转处罚与工作、再关周期：周期关闭后不再有 pending 阻断残留。
    let closed_sanction_id: Option<Uuid> = sqlx::query_scalar(
        "UPDATE public.consignment_sanctions SET state = 'cycle_closed', version = version + 1
         WHERE buyer_user_id = $1 AND cycle_id = $2 AND state = 'pending'
         RETURNING id",
    )
    .bind(buyer.as_uuid())
    .bind(closed_cycle_id)
    .fetch_optional(&mut *conn)
    .await?;
    if let Some(sanction_id) = closed_sanction_id {
        cancel_open_work(conn, sanction_id).await?;
    }
    sqlx::query(
        "UPDATE public.consignment_refusal_cycles
         SET state = 'closed', closed_at = $2, close_reason = $3
         WHERE id = $1",
    )
    .bind(closed_cycle_id)
    .bind(now)
    .bind(CLOSE_REASON_UNBLOCKED)
    .execute(&mut *conn)
    .await?;
    let opened_cycle_id = open_cycle(conn, buyer, now).await?;
    Ok(CycleTransition {
        closed_cycle_id: Some(closed_cycle_id),
        opened_cycle_id,
        closed_sanction_id,
    })
}
