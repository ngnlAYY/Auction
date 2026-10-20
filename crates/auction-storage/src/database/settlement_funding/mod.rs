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
pub use deliveries::{
    finish_funded_delivery, reserve_funded_delivery, return_funded_delivery, start_funded_delivery,
};
pub use obligations::{
    fund_collected_payment, fund_internal_purchase, refund_funded_purchase, retain_funded_seller,
};

use auction_application::{FundingError, FundingMutation};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

/// SQL失败仅暴露安全分类；事务owner负责重试40001/40P01并保留原命令。
fn storage(error: sqlx::Error) -> FundingError {
    if matches!(&error, sqlx::Error::Database(database) if matches!(database.code().as_deref(),Some("40001"|"40P01")))
    {
        FundingError::SerializationConflict
    } else {
        FundingError::Storage
    }
}

/// A的SQLx错误先按SQLSTATE分类，业务拒绝才映射Unproven。
fn evidence_error(error: crate::MailEvidenceTransactionError) -> FundingError {
    match error {
        crate::MailEvidenceTransactionError::Sqlx(error) => storage(error),
        crate::MailEvidenceTransactionError::Rejected(_) => FundingError::Unproven,
    }
}

fn invalid<T>(_: T) -> FundingError {
    FundingError::Invalid
}
