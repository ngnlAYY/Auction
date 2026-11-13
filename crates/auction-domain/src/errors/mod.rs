//! 领域错误。

use thiserror::Error;

use crate::{auction::AuctionStatus, bidding::BidAmount, proxy::ProxyStatus};

#[derive(Debug, Error, PartialEq, Eq)]
pub enum DomainError {
    #[error("auction title must not be empty")]
    EmptyTitle,

    #[error("invalid auction transition from {from:?} to {to:?}")]
    InvalidTransition { from: AuctionStatus, to: AuctionStatus },

    #[error("invalid amount: {value}")]
    InvalidAmount { value: i64 },

    #[error("amount arithmetic overflowed")]
    AmountOverflow,

    #[error("invalid auction rules: {0}")]
    InvalidRules(&'static str),

    #[error("invalid auction schedule: {0}")]
    InvalidSchedule(&'static str),

    #[error("bid amount is below the minimum of {minimum}")]
    BidTooLow { minimum: BidAmount },

    #[error("auction is past its effective deadline")]
    AuctionClosed,

    #[error("invalid auction cancellation metadata")]
    InvalidCancellationMetadata,

    #[error("invalid lot bidding state: {0}")]
    InvalidBidState(&'static str),

    #[error("bidder is already leading the lot")]
    AlreadyLeading,

    #[error("bidder already owns the active proxy authorization")]
    ActiveProxyExists,

    #[error("invalid proxy transition from {from:?} to {to:?}")]
    InvalidProxyTransition { from: ProxyStatus, to: ProxyStatus },

    #[error("proxy maximum must be lowered while it is active")]
    InvalidProxyMaximum,

    #[error("proxy authorization is not active")]
    ProxyAuthorizationNotActive,

    #[error("proxy maximum was not strictly lowered")]
    ProxyMaximumNotLower,

    #[error("proxy maximum is below the binding amount")]
    ProxyMaximumBelowBinding,

    #[error("proxy re-entry maximum was not raised")]
    ProxyReentryMaximumNotHigher,

    #[error("proxy bidding is disabled for this auction")]
    ProxyDisabled,

    #[error("proxy re-entry quota has been exhausted")]
    ProxyOutbidLimit,

    #[error("slow proxy is not allowed in the final phase")]
    ProxySlowFinal,

    #[error("state version overflowed")]
    StateVersionOverflow,

    #[error("invalid settlement policy: {0}")]
    InvalidSettlementPolicy(&'static str),

    #[error("invalid settlement facts: {0}")]
    InvalidSettlementFacts(&'static str),

    #[error("invalid settlement obligation: {0}")]
    InvalidSettlementObligation(&'static str),

    #[error("settlement amount arithmetic overflowed")]
    SettlementAmountOverflow,
}
