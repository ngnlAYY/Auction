use async_trait::async_trait;
use chrono::{DateTime, Utc};
use thiserror::Error;

use auction_domain::{Auction, AuctionId};

#[derive(Debug, Error)]
pub enum RepositoryError {
    #[error("repository operation failed: {0}")]
    Failed(String),
}

#[async_trait]
pub trait AuctionRepository: Send + Sync {
    async fn insert(&self, auction: Auction) -> Result<(), RepositoryError>;
    async fn get(&self, id: AuctionId) -> Result<Option<Auction>, RepositoryError>;
}

pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}
