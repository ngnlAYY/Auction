use std::sync::Arc;

use auction_domain::{Auction, AuctionId};

use crate::{ApplicationError, AuctionRepository, Clock};

#[derive(Debug, Clone)]
pub struct CreateAuctionCommand {
    pub title: String,
}

pub struct AuctionService<R, C> {
    repository: Arc<R>,
    clock: Arc<C>,
}

impl<R, C> AuctionService<R, C>
where
    R: AuctionRepository,
    C: Clock,
{
    pub fn new(repository: Arc<R>, clock: Arc<C>) -> Self {
        Self { repository, clock }
    }

    pub async fn create(&self, command: CreateAuctionCommand) -> Result<Auction, ApplicationError> {
        let auction = Auction::draft(AuctionId::new(), command.title, self.clock.now())?;
        self.repository.insert(auction.clone()).await?;
        Ok(auction)
    }
}
