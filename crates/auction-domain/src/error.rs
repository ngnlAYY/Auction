use thiserror::Error;

use crate::AuctionStatus;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum DomainError {
    #[error("auction title must not be empty")]
    EmptyTitle,

    #[error("invalid auction transition from {from:?} to {to:?}")]
    InvalidTransition { from: AuctionStatus, to: AuctionStatus },
}
