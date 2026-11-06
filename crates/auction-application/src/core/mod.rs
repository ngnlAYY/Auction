//! core 上下文公开 API。

pub use crate::errors::{ApplicationError, MailProcessingRetryability, SessionError, diagnostics};
pub use crate::operator_audit::{CliOperatorIdentity, cli_audit_metadata};
pub use crate::ports::core::{
    AuctionRepository, Clock, CommandInsert, CommandLedger, CommandLedgerError,
    CompetitionCommandAuditContext, CompetitionCommandAuditKind, CompetitionProxyTransitionRecord,
    CompetitionSnapshot, CompetitionTransaction, CompetitionTransactionError, ProxyUsageContext,
    RepositoryError, RequestHash, SlowProxyExecutionContext, StoredCommand, TransactionFailure,
    TransactionFuture, TransactionIsolation, TransactionPolicy, TransactionPolicyError,
    TransactionRunner, lower_hex,
};
