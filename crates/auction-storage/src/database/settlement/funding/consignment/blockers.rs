//! 现金退件的 UID 级 opening 阻断（D9 §4.2）。
//!
//! 保护不变量 2、4、6：任何可能已 Take、尚未完成资金消费的现金退件，阻断该 UID 的现金 opening；
//! 阻断最迟随现金 Take 的 `write_started` 同事务建立，只随原资金消费同事务解除（见 `returns.rs`）。
//! 缺消费行、未知旧退件、起写后结果未知都不能证明安全。opening 的最终写在 UID 账户锁内重验。

use auction_application::settlement::FundingError;
use auction_domain::consignment::funding::{
    BlockedLeg, FundingMode, FundingRuleError, OpeningBlock, check_opening_gate,
};
use sqlx::{Postgres, Row};
use uuid::Uuid;

use super::contract_allocations::{mode_code, parse_mode};
use crate::database::TrackedTransaction as Transaction;
use crate::database::settlement::funding::account::lock_funding_account;
use crate::database::settlement::funding::{invalid, storage};

/// 阻断腿的持久码。
pub(super) const fn leg_code(leg: BlockedLeg) -> &'static str {
    match leg {
        BlockedLeg::Refund => "cash_refund",
        BlockedLeg::Payout => "cash_payout",
        BlockedLeg::CarrierPrincipal => "cash_principal",
    }
}

fn parse_leg(value: &str) -> Result<BlockedLeg, FundingError> {
    [BlockedLeg::Refund, BlockedLeg::Payout, BlockedLeg::CarrierPrincipal]
        .into_iter()
        .find(|leg| leg_code(*leg) == value)
        .ok_or(FundingError::Unproven)
}

/// Take 起写的同一事务内建立阻断，返回阻断 ID；同 `(intake, leg)` 重放返回原 ID。
///
/// 调用方（退件接管的起写事务）必须：已取得 funding worlds 锁，且在本事务里先写入该 intake 唯一
/// Take 工作的 `mail_disposition_write_starts` 行再调用——阻断以它为外键，并要求它由本事务写入。
/// 本原语取 UID 账户锁并推进账户版本，使并发的 opening 重读阻断（旧预览与旧版本 CAS 同时作废）。
/// 起写工作 ID 在事务内取自 `consignment_return_intakes.take_work_id`，不由调用方传入（R5 §3.8 的签名）。
#[allow(dead_code, reason = "由第 6 期 6D5 的 Take 起写事务调用，集成前尚无调用方")]
pub(crate) async fn open_cash_return_block_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    block: &OpeningBlock,
) -> Result<Uuid, FundingError> {
    if block.uid.trim().is_empty()
        || block.amount <= 0
        || block.intake_id.is_nil()
        || block.original_delivery_id.is_nil()
    {
        return Err(FundingError::Invalid);
    }
    let account = lock_funding_account(tx, &block.uid).await?.ok_or(FundingError::Unavailable)?;
    // 该 intake 唯一的 Take 工作即阻断要绑定的起写；intake 不存在或尚无 Take 工作时没有可绑定的起写。
    let write_start_work_id: Uuid = sqlx::query_scalar(
        r#"SELECT take_work_id
           FROM public.consignment_return_intakes
           WHERE id=$1 AND leg=$2 AND original_delivery_id=$3 AND take_work_id IS NOT NULL"#,
    )
    .bind(block.intake_id)
    .bind(leg_code(block.leg))
    .bind(block.original_delivery_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage)?
    .ok_or(FundingError::Conflict)?;
    let existing = sqlx::query(
        r#"SELECT id, uid, original_delivery_id, amount, original_mode, opened_in_write_start
           FROM public.consignment_cash_return_blocks
           WHERE intake_id=$1 AND leg=$2
           FOR UPDATE"#,
    )
    .bind(block.intake_id)
    .bind(leg_code(block.leg))
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage)?;
    if let Some(row) = existing {
        let same = row.try_get::<String, _>("uid").map_err(storage)? == block.uid
            && row.try_get::<Uuid, _>("original_delivery_id").map_err(storage)?
                == block.original_delivery_id
            && row.try_get::<i64, _>("amount").map_err(storage)? == block.amount
            && parse_mode(&row.try_get::<String, _>("original_mode").map_err(storage)?)?
                == block.original_mode
            && row.try_get::<Uuid, _>("opened_in_write_start").map_err(storage)?
                == write_start_work_id;
        return if same { row.try_get("id").map_err(storage) } else { Err(FundingError::Conflict) };
    }
    // 账户模式不可切换：阻断记录的原模式必须就是账户的模式。
    let account_mode =
        if account.records_only() { FundingMode::RecordOnly } else { FundingMode::StrictCash };
    if account_mode != block.original_mode {
        return Err(FundingError::Conflict);
    }
    // 阻断绑定该 intake、其唯一 Take 工作的起写（必须由本事务写入）、原支出腿与成交的资金 UID。
    let bound: bool = sqlx::query_scalar(
        r#"SELECT EXISTS(
               SELECT 1
               FROM public.consignment_return_intakes AS intake
               JOIN public.consignment_sales AS sale ON sale.contract_id = intake.contract_id
               JOIN public.mail_disposition_write_starts AS started
                 ON started.work_id = intake.take_work_id
               WHERE intake.id=$1 AND intake.leg=$2 AND intake.original_delivery_id=$3
                 AND sale.funding_uid=$4
                 AND started.xmin = pg_current_xact_id()::xid
           )"#,
    )
    .bind(block.intake_id)
    .bind(leg_code(block.leg))
    .bind(block.original_delivery_id)
    .bind(&block.uid)
    .fetch_one(&mut **tx)
    .await
    .map_err(storage)?;
    if !bound {
        return Err(FundingError::Conflict);
    }
    let block_id = Uuid::new_v4();
    sqlx::query(
        r#"INSERT INTO public.consignment_cash_return_blocks
               (id, uid, intake_id, leg, original_delivery_id, amount, original_mode, state,
                opened_in_write_start)
           VALUES ($1, $2, $3, $4, $5, $6, $7, 'open', $8)"#,
    )
    .bind(block_id)
    .bind(&block.uid)
    .bind(block.intake_id)
    .bind(leg_code(block.leg))
    .bind(block.original_delivery_id)
    .bind(block.amount)
    .bind(mode_code(block.original_mode))
    .bind(write_start_work_id)
    .execute(&mut **tx)
    .await
    .map_err(storage)?;
    sqlx::query(
        r#"UPDATE public.settlement_funding_accounts
           SET state_version=state_version+1, updated_at=clock_timestamp()
           WHERE uid=$1"#,
    )
    .bind(account.uid())
    .execute(&mut **tx)
    .await
    .map_err(storage)?;
    Ok(block_id)
}

/// opening 最终写的 UID 级门：本 UID 存在任一未核清的现金退件阻断即拒绝（含零额建界、调整与结转）。
///
/// 调用方已持该 UID 的账户锁；别的 UID 的阻断不影响本 UID。
pub(crate) async fn ensure_opening_allowed(
    tx: &mut Transaction<'_, Postgres>,
    uid: &str,
) -> Result<(), FundingError> {
    let rows = sqlx::query(
        r#"SELECT intake_id, leg, original_delivery_id, amount, original_mode
           FROM public.consignment_cash_return_blocks
           WHERE uid=$1 AND state='open'"#,
    )
    .bind(uid)
    .fetch_all(&mut **tx)
    .await
    .map_err(storage)?;
    let mut blocks = Vec::with_capacity(rows.len());
    for row in rows {
        let leg: String = row.try_get("leg").map_err(storage)?;
        let mode: String = row.try_get("original_mode").map_err(storage)?;
        blocks.push(
            OpeningBlock::new(
                uid.to_owned(),
                row.try_get("intake_id").map_err(storage)?,
                parse_leg(&leg)?,
                row.try_get("original_delivery_id").map_err(storage)?,
                row.try_get("amount").map_err(storage)?,
                parse_mode(&mode)?,
            )
            .map_err(invalid)?,
        );
    }
    match check_opening_gate(uid, &blocks) {
        Ok(()) => Ok(()),
        // 未核清的现金退件责任：不是资金不足，是外部结果尚未证明。
        Err(FundingRuleError::OpeningBlocked) => Err(FundingError::Unproven),
        Err(error) => Err(invalid(error)),
    }
}
