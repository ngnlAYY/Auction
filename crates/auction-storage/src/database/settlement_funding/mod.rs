//! 唯一现金账本owner；事务helper不提交、不外呼、不决定履约状态。

mod account;
mod cash_gate;
mod classification;
mod deliveries;
mod ledger;
mod legacy;
mod obligations;
mod policy;
mod preview;
mod receipt;
mod reconciliation;
mod record_only;

pub use account::{
    LockedFundingAccount, check_funding_cash, check_funding_evidence,
    funding_requires_authorization, lock_funded_delivery_account, lock_funding_account,
};
pub use deliveries::{finish_funded_delivery, reserve_funded_delivery, start_funded_delivery};
pub use obligations::{
    fund_collected_payment, fund_internal_purchase, refund_funded_purchase, retain_funded_seller,
};

use auction_application::{FundingError, FundingMutation, SettlementInternalEvidenceSnapshot};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

/// SQL失败仅暴露安全分类；事务owner负责重试40001/40P01并保留原命令。
/// 旧自寄修复与内部履约复用同一映射，不另建副本。
pub(crate) fn storage(error: sqlx::Error) -> FundingError {
    if crate::database::is_serialization_conflict(&error) {
        FundingError::SerializationConflict
    } else {
        FundingError::Storage
    }
}

/// 证据事务的SQLx错误先按SQLSTATE分类，业务拒绝才映射Unproven。
pub(crate) fn evidence_error(error: crate::MailEvidenceTransactionError) -> FundingError {
    match error {
        crate::MailEvidenceTransactionError::Sqlx(error) => storage(error),
        crate::MailEvidenceTransactionError::Rejected(_) => FundingError::Unproven,
    }
}

fn invalid<T>(_: T) -> FundingError {
    FundingError::Invalid
}

/// 资金与内部履约共用的锁前缀：按持久 code 排序锁定全部 world，跨世界不拆事务。
pub(crate) async fn lock_funding_worlds(
    tx: &mut Transaction<'_, Postgres>,
) -> Result<(), FundingError> {
    lock_rows(tx, "SELECT id FROM public.worlds ORDER BY id FOR UPDATE").await
}

/// 低频全局资金命令在资金账户之后锁定完整财务责任集合，防跨 lot 重复认领。对账、旧实收接管、
/// 仅记账初始化与旧自寄修复共用这一份顺序，凭据锁固定在 equipment 与 users 之间，各入口不能漂移成
/// 相反锁序；修复路径也不会把当前 manual 行改成可被外发领取的任务。
pub(crate) async fn lock_funding_liabilities(
    tx: &mut Transaction<'_, Postgres>,
    evidence: &SettlementInternalEvidenceSnapshot,
) -> Result<(), FundingError> {
    for query in [
        "SELECT id FROM public.auctions ORDER BY id FOR UPDATE",
        "SELECT id FROM public.settlement_recoveries ORDER BY id FOR UPDATE",
        "SELECT id FROM public.lots ORDER BY id FOR UPDATE",
        "SELECT id FROM public.equipment ORDER BY id FOR UPDATE",
    ] {
        lock_rows(tx, query).await?;
    }
    crate::lock_mail_evidence_credentials(tx, evidence).await.map_err(evidence_error)?;
    for query in [
        "SELECT id FROM public.users ORDER BY id FOR UPDATE",
        "SELECT id FROM public.settlement_obligations ORDER BY id FOR UPDATE",
        "SELECT id FROM public.settlement_recovery_actions ORDER BY id FOR UPDATE",
        "SELECT id FROM public.deliveries ORDER BY id FOR UPDATE",
        "SELECT obligation_id FROM public.settlement_obligation_fulfillments
         ORDER BY obligation_id FOR UPDATE",
    ] {
        lock_rows(tx, query).await?;
    }
    Ok(())
}

/// 只需完成有序加锁，execute 仍完整执行 SELECT，但不保留随后丢弃的全表行集。
async fn lock_rows(
    tx: &mut Transaction<'_, Postgres>,
    query: &'static str,
) -> Result<(), FundingError> {
    sqlx::query(query).execute(&mut **tx).await.map_err(storage)?;
    Ok(())
}
