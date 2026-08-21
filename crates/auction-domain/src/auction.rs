use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{AuctionId, DomainError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuctionStatus {
    Draft,
    Preview,
    Live,
    Final,
    Settling,
    Settled,
    Cancelled,
}

impl AuctionStatus {
    fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Draft, Self::Preview)
                | (Self::Preview, Self::Live)
                | (Self::Live, Self::Final)
                | (Self::Final, Self::Settling)
                | (Self::Settling, Self::Settled)
                | (Self::Draft, Self::Cancelled)
                | (Self::Preview, Self::Cancelled)
                | (Self::Live, Self::Cancelled)
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Auction {
    pub id: AuctionId,
    pub title: String,
    pub status: AuctionStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Auction {
    pub fn draft(
        id: AuctionId,
        title: impl Into<String>,
        now: DateTime<Utc>,
    ) -> Result<Self, DomainError> {
        let title = title.into();
        if title.trim().is_empty() {
            return Err(DomainError::EmptyTitle);
        }

        Ok(Self { id, title, status: AuctionStatus::Draft, created_at: now, updated_at: now })
    }

    pub fn preview(&mut self, now: DateTime<Utc>) -> Result<(), DomainError> {
        self.transition(AuctionStatus::Preview, now)
    }

    pub fn go_live(&mut self, now: DateTime<Utc>) -> Result<(), DomainError> {
        self.transition(AuctionStatus::Live, now)
    }

    pub fn enter_final(&mut self, now: DateTime<Utc>) -> Result<(), DomainError> {
        self.transition(AuctionStatus::Final, now)
    }

    pub fn begin_settlement(&mut self, now: DateTime<Utc>) -> Result<(), DomainError> {
        self.transition(AuctionStatus::Settling, now)
    }

    pub fn settle(&mut self, now: DateTime<Utc>) -> Result<(), DomainError> {
        self.transition(AuctionStatus::Settled, now)
    }

    pub fn cancel(&mut self, now: DateTime<Utc>) -> Result<(), DomainError> {
        self.transition(AuctionStatus::Cancelled, now)
    }

    fn transition(&mut self, next: AuctionStatus, now: DateTime<Utc>) -> Result<(), DomainError> {
        if !self.status.can_transition_to(next) {
            return Err(DomainError::InvalidTransition { from: self.status, to: next });
        }

        self.status = next;
        self.updated_at = now;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use chrono::{DateTime, Utc};

    use super::*;

    #[test]
    fn draft_follows_the_initial_lifecycle() {
        let now = DateTime::<Utc>::from(SystemTime::UNIX_EPOCH);
        let mut auction = Auction::draft(AuctionId::new(), "Example", now).unwrap();

        auction.preview(now).unwrap();
        auction.go_live(now).unwrap();
        auction.enter_final(now).unwrap();
        auction.begin_settlement(now).unwrap();
        auction.settle(now).unwrap();

        assert_eq!(auction.status, AuctionStatus::Settled);
    }

    #[test]
    fn invalid_transitions_are_rejected() {
        let now = DateTime::<Utc>::from(SystemTime::UNIX_EPOCH);
        let mut auction = Auction::draft(AuctionId::new(), "Example", now).unwrap();

        let error = auction.settle(now).unwrap_err();

        assert!(matches!(error, DomainError::InvalidTransition { .. }));
    }
}
