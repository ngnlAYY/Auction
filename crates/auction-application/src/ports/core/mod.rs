//! core 上下文。

pub(crate) mod clock;
pub(crate) mod competition;
pub(crate) mod encoding;
pub(crate) mod ledger;
pub(crate) mod policy;
pub(crate) mod repository;

pub use clock::Clock;
pub use competition::{
    CommandInsert, CompetitionCommandAuditContext, CompetitionCommandAuditKind,
    CompetitionProxyTransitionRecord, CompetitionSnapshot, CompetitionTransaction,
    CompetitionTransactionError, ProxyUsageContext, SlowProxyExecutionContext, TransactionFailure,
    TransactionFuture, TransactionRunner,
};
pub(crate) use encoding::for_each_legacy_go_json_segment;
pub use encoding::lower_hex;
pub use ledger::{CommandLedger, CommandLedgerError, RequestHash, StoredCommand};
pub use policy::{TransactionIsolation, TransactionPolicy, TransactionPolicyError};
pub use repository::{AuctionRepository, RepositoryError};
