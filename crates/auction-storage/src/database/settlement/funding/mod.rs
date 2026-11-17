//! 唯一现金账本owner；事务helper不提交、不外呼、不决定履约状态。
//!
//! 现金门能力由 `delivery::scheduler` 唯一实现；资金owner不自行授权V1。
//! 启用资金账户的部署前提由 docs/guide/settlement.md 的“资金账户”一节维护。

mod account;
mod classification;
mod deliveries;
pub(crate) mod diagnostics;
mod ledger;
mod obligations;
mod policy;
mod preview;
mod receipt;
mod reconciliation;
mod record_only;
mod returns;

pub use account::{
    LockedFundingAccount, check_funding_evidence, funding_requires_authorization,
    lock_funded_delivery_account, lock_funding_account,
};
pub use deliveries::{finish_funded_delivery, reserve_funded_delivery, start_funded_delivery};
pub use obligations::{
    fund_collected_payment, fund_equipment_loss, fund_internal_purchase, refund_funded_purchase,
    retain_funded_seller,
};
pub use returns::{FundingReturnBinding, funded_delivery_spent, return_funded_delivery};

use auction_application::settlement::{FundingError, SettlementInternalEvidenceSnapshot};
use sqlx::Postgres;

use crate::database::TrackedTransaction as Transaction;

/// SQL失败仅暴露安全分类；事务owner负责重试40001/40P01并保留原命令。
/// 历史自寄修复证明与内部履约复用同一映射，不另建副本。
pub(crate) fn storage(error: sqlx::Error) -> FundingError {
    if crate::database::observed_serialization_conflict(&error) {
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
    lock_rows(tx, &["SELECT id FROM public.worlds ORDER BY id FOR UPDATE"]).await.map_err(storage)
}

/// 低频全局资金命令在资金账户之后锁定完整财务责任集合，防跨 lot 重复认领。现金对账、仅记账
/// 初始化、人工 Sent 证明与装备损失共用这一份顺序：凭据锁固定在 equipment 与 users 之间，没有
/// 凭据的入口直接衔接前后两段；各入口不能漂移成相反锁序（不变量 2）。
pub(crate) async fn lock_funding_liabilities(
    tx: &mut Transaction<'_, Postgres>,
    evidence: &SettlementInternalEvidenceSnapshot,
) -> Result<(), FundingError> {
    lock_liabilities_before_credentials(tx).await.map_err(storage)?;
    crate::lock_mail_evidence_credentials(tx, evidence).await.map_err(evidence_error)?;
    lock_liabilities_after_credentials(tx).await.map_err(storage)
}

/// 财务责任集合排在凭据锁之前的一段；SQL 错误原样返回，由各入口按自身错误类型分类与重试。
pub(crate) async fn lock_liabilities_before_credentials(
    tx: &mut Transaction<'_, Postgres>,
) -> Result<(), sqlx::Error> {
    lock_rows(
        tx,
        &[
            "SELECT id FROM public.auctions ORDER BY id FOR UPDATE",
            "SELECT id FROM public.settlement_recoveries ORDER BY id FOR UPDATE",
            "SELECT id FROM public.lots ORDER BY id FOR UPDATE",
            "SELECT id FROM public.equipment ORDER BY id FOR UPDATE",
        ],
    )
    .await
}

/// 财务责任集合排在凭据锁之后的一段。
pub(crate) async fn lock_liabilities_after_credentials(
    tx: &mut Transaction<'_, Postgres>,
) -> Result<(), sqlx::Error> {
    lock_rows(
        tx,
        &[
            "SELECT id FROM public.users ORDER BY id FOR UPDATE",
            "SELECT id FROM public.settlement_obligations ORDER BY id FOR UPDATE",
            "SELECT id FROM public.settlement_recovery_actions ORDER BY id FOR UPDATE",
            "SELECT id FROM public.deliveries ORDER BY id FOR UPDATE",
            r#"SELECT obligation_id FROM public.settlement_obligation_fulfillments
               ORDER BY obligation_id FOR UPDATE"#,
        ],
    )
    .await
}

/// 只需完成有序加锁，execute 仍完整执行 SELECT，但不保留随后丢弃的全表行集。
async fn lock_rows(
    tx: &mut Transaction<'_, Postgres>,
    queries: &[&'static str],
) -> Result<(), sqlx::Error> {
    for &query in queries {
        sqlx::query(query).execute(&mut **tx).await?;
    }
    Ok(())
}
