use std::future::Future;

use thiserror::Error;

use auction_domain::{Auction, AuctionId};

#[derive(Debug, Error)]
pub enum RepositoryError {
    #[error("repository operation failed: {0}")]
    Failed(String),
}

pub trait AuctionRepository: Send + Sync {
    fn insert(&self, auction: Auction) -> impl Future<Output = Result<(), RepositoryError>> + Send;

    fn get(
        &self,
        id: AuctionId,
    ) -> impl Future<Output = Result<Option<Auction>, RepositoryError>> + Send;
}
