//! 严格现金的新鲜同 UID 现金复验（D9 §4.2 第 1 步）。
//!
//! 保护不变量 2、6：在 UID 账户锁内、以数据库时钟确认观察仍在有效窗口内且属于本 UID，并要求观察到的
//! Main 现金仍覆盖该 UID 全部未支出占用加本笔新发行额 U（已含的 A 早在原 opening 里，不增现金）。
//! 现金已被花掉、观察过期或不属于本 UID，一律整笔失败关闭，不冻结账户、不降级放行。

use auction_application::settlement::{
    CertifyCod, FundingError, INTERNAL_EVIDENCE_MAX_AGE_SECONDS,
};
use auction_domain::consignment::funding::FundingMode;
use sqlx::Postgres;

use crate::database::TrackedTransaction as Transaction;
use crate::database::settlement::funding::account::LockedFundingAccount;
use crate::database::settlement::funding::storage;

/// `record_only` 不读个人余额，直接放行（端口已保证它不带观察）；严格现金逐项复验。
pub(super) async fn verify_fresh_cash(
    tx: &mut Transaction<'_, Postgres>,
    account: &LockedFundingAccount,
    command: &CertifyCod,
    new_issue: i64,
) -> Result<(), FundingError> {
    if command.mode == FundingMode::RecordOnly {
        return Ok(());
    }
    let cash = command.cash.as_ref().ok_or(FundingError::Invalid)?;
    if cash.uid != account.uid() || cash.credits < 0 {
        return Err(FundingError::Unproven);
    }
    let fresh: bool = sqlx::query_scalar(
        r#"WITH sql_clock AS MATERIALIZED (SELECT clock_timestamp() AS at)
           SELECT (SELECT at FROM sql_clock) >= $1 AND (SELECT at FROM sql_clock) < $2
                  AND $2 - $1 <= $3::bigint * interval '1 second'"#,
    )
    .bind(cash.observed_at)
    .bind(cash.expires_at)
    .bind(INTERNAL_EVIDENCE_MAX_AGE_SECONDS)
    .fetch_one(&mut **tx)
    .await
    .map_err(storage)?;
    if !fresh {
        return Err(FundingError::Unproven);
    }
    let (occupied, unresolved): (i64, bool) = sqlx::query_as(
        r#"SELECT COALESCE(sum(amount) FILTER (WHERE state <> 'spent'), 0)::bigint,
                  COALESCE(bool_or(state IN ('unknown', 'in_flight')), false)
           FROM public.settlement_funding_allocations
           WHERE uid = $1"#,
    )
    .bind(account.uid())
    .fetch_one(&mut **tx)
    .await
    .map_err(storage)?;
    let required = occupied.checked_add(new_issue).ok_or(FundingError::Invalid)?;
    // 未收口的外发占用使余额无法与账本比对，与拍卖资金门一致，暂不放行。
    if unresolved || cash.credits < required {
        return Err(FundingError::Unavailable);
    }
    Ok(())
}
