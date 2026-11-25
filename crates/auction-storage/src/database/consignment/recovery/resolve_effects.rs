//! 退回处置的应用效果（7A-2）：在记录处置的同一事务里封闭原买方交割并把挂单转为下架。
//!
//! 保护不变量 2、3、4、6：效果与处置行、成功回执同事务提交，任一步失败整笔回滚。
//! - 原买方交割：`close_at_limit` / `terminate_unpaid` 调 6B 资金 owner 的 `cancel_unpaid`（零资金分录），
//!   写 Sale 封闭事实；其余处置不封闭。
//! - 挂单：`held_unpaid_recovered` 把 `sale_pending` 转为 `held_off_market(unpaid_recovered)`，版本、
//!   `hold_set` 事件与回执里的挂单变更同事务；000137 的挂单守卫要求同事务的处置行与封闭事实。
//! - 退款与本金：现金退件（退款、卖款、载体本金）的回补已由 6D5 的消费事务按原支出一次性完成；
//!   “转退款”（`refund_p`）同事务调用 6B 单写者 `convert_refund_in_tx`（锁内复验相反付款），转换后追加
//!   `refund_closed` 封闭事实与订单阶段事件。它的前置（原款在账）当前没有可读事实，矩阵恒留案，读口就绪前
//!   实际走不到这里；相反付款未决时整笔回滚、不留任何资金或处置痕迹。转已付退回下架（`held_paid_returned`）
//!   的挂单边本期没有 owner，失败关闭为证据冲突、不改任何行。
//!
//! 事实先在 [`plan`] 里全部核对（零写入），核对不通过返回确定拒绝，之后 [`apply`] 只写。

use auction_application::consignment::{ConsignmentError, DepositBusinessRejection, ResolveReturn};
use auction_application::settlement::{ConvertRefund, FundingMutation};
use auction_domain::consignment::ListingState;
use auction_domain::consignment::recovery::{
    BuyerRightEffect, ListingEffect, RefundEffect, ReturnDispositionDecision,
};
use auction_time::Timestamp;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::commands::session::DepositTransaction;
use crate::database::consignment::listings::projection::{LockedListing, lock_listings};
use crate::database::consignment::reads::orders::record_order_stage_change_in_tx;
use crate::database::consignment::recovery::refund_closure::{
    record_refund_closure, refund_command,
};
use crate::database::consignment::returns::funding_error;
use crate::database::settlement::funding::consignment::cancel_unpaid::{
    CancelUnpaid, cancel_unpaid_in_tx,
};
use crate::database::settlement::funding::consignment::refund_conversion::{
    RefundInTx, convert_refund_in_tx,
};

/// 6B 资金命令 ID 的派生域：同一条处置命令永远得到同一个资金命令 ID，重试不会产生第二笔转换。
const REFUND_COMMAND_DOMAIN: &[u8] = b"consignment.resolve_return.refund.v1";

/// 已核对、待写入的效果。
pub(super) struct EffectPlan {
    cancel_unpaid: bool,
    hold: Option<LockedListing>,
    refund: Option<ConvertRefund>,
}

/// 挂单被本处置改动后的版本，写进回执供事件核对。
#[derive(Debug, Clone, Copy)]
pub(super) struct ListingChange {
    pub(super) listing_id: Uuid,
    pub(super) listing_version: u64,
}

impl EffectPlan {
    /// 本处置将改动的挂单版本（若转下架）；回执先于挂单事件入账，所以版本要在写入前就确定。
    pub(super) fn listing_change(&self) -> Option<ListingChange> {
        self.hold.as_ref().map(|listing| ListingChange {
            listing_id: listing.listing_id,
            listing_version: listing.snapshot.state_version.value() + 1,
        })
    }
}

/// 核对效果的前置事实；`None` 表示事实不成立（确定拒绝，调用方尚未写任何行）。
pub(super) async fn plan(
    connection: &mut PgConnection,
    command: &ResolveReturn,
    contract_id: Uuid,
    listing_id: Uuid,
    decision: ReturnDispositionDecision,
) -> Result<Option<EffectPlan>, WriteError> {
    if decision.listing == ListingEffect::HeldPaidReturned {
        return Ok(None);
    }
    let refund = if decision.refund == RefundEffect::RefundP {
        let mutation = FundingMutation {
            command_id: Uuid::new_v5(&command.command_id.as_uuid(), REFUND_COMMAND_DOMAIN),
            request_digest: command.request_hash.as_bytes(),
        };
        // 只有 Sale 有买家退款义务；没有时前提不成立，确定拒绝。
        match refund_command(connection, contract_id, mutation).await? {
            Some(convert) => Some(convert),
            None => return Ok(None),
        }
    } else {
        None
    };
    let cancel_unpaid = matches!(
        decision.buyer_right,
        BuyerRightEffect::CloseAtLimit | BuyerRightEffect::TerminateUnpaid
    );
    let hold = if decision.listing == ListingEffect::HeldUnpaidRecovered {
        let listing = lock_listings(connection, &[listing_id]).await?.into_iter().next();
        // 下架只发生在仍在等待履约的 `sale_pending` 挂单上；其他状态（季末关闭、已成交……）
        // 说明这份处置的前提已不存在，不能把挂单改成下架。
        match listing {
            Some(listing)
                if cancel_unpaid && listing.snapshot.state == ListingState::SalePending =>
            {
                Some(listing)
            }
            _ => return Ok(None),
        }
    } else {
        None
    };
    Ok(Some(EffectPlan { cancel_unpaid, hold, refund }))
}

/// 写入效果；调用方已写入成功回执（挂单事件的命令外键要求命令行先于事件）。
pub(super) async fn apply(
    tx: &mut DepositTransaction<'_, '_>,
    command_id: Uuid,
    contract_id: Uuid,
    disposition_id: Uuid,
    plan: EffectPlan,
    now: Timestamp,
) -> Result<(), WriteError> {
    if plan.cancel_unpaid {
        cancel_unpaid_in_tx(tx.tracked(), &CancelUnpaid { contract_id, disposition_id })
            .await
            .map_err(funding_error)?;
    }
    if let Some(listing) = &plan.hold {
        hold_listing(tx.connection(), command_id, listing, now).await?;
    }
    if let Some(convert) = &plan.refund {
        // 不变量 4：相反付款（卖款可能已发）在 UID 账户锁内复验；未决时整笔回滚，禁止双派。
        match convert_refund_in_tx(tx.tracked(), convert).await.map_err(funding_error)? {
            RefundInTx::Converted(_) => {
                let basis = format!("refund-conversion:{}", convert.mutation.command_id);
                record_refund_closure(tx.tracked(), contract_id, &basis)
                    .await
                    .map_err(funding_error)?;
            }
            RefundInTx::OppositePaymentOpen => {
                return Err(WriteError::Application(ConsignmentError::Deposit(
                    DepositBusinessRejection::OppositePaymentOpen,
                )));
            }
        }
    }
    if plan.cancel_unpaid || plan.refund.is_some() {
        // 订单阶段事件、权利刷新与封闭事实同事务（不变量 3）。
        record_order_stage_change_in_tx(tx.connection(), contract_id, now).await?;
    }
    Ok(())
}

/// `sale_pending → held_off_market(unpaid_recovered)`：同版本 `hold_set` 事件先于挂单更新写入。
async fn hold_listing(
    connection: &mut PgConnection,
    command_id: Uuid,
    listing: &LockedListing,
    now: Timestamp,
) -> Result<(), WriteError> {
    let current = listing.snapshot.state_version.value();
    let version = current + 1;
    sqlx::query(
        "INSERT INTO public.consignment_events
            (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
             to_state, reason_code, price, occurred_at)
         VALUES ($1, $2, $3, 'worker', 'hold_set', 'sale_pending', 'held_off_market',
                 'unpaid_recovered', $4, $5)",
    )
    .bind(listing.listing_id)
    .bind(version.cast_signed())
    .bind(command_id)
    .bind(listing.snapshot.price.value())
    .bind(now)
    .execute(&mut *connection)
    .await?;
    let moved = sqlx::query(
        "UPDATE public.consignment_listings
         SET state = 'held_off_market', state_version = $3, reason_code = 'unpaid_recovered',
             updated_at = $4
         WHERE id = $1 AND state_version = $2 AND state = 'sale_pending'",
    )
    .bind(listing.listing_id)
    .bind(current.cast_signed())
    .bind(version.cast_signed())
    .bind(now)
    .execute(connection)
    .await?;
    if moved.rows_affected() != 1 {
        return Err(WriteError::invariant("consignment_resolve_listing_version"));
    }
    Ok(())
}
