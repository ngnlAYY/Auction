//! Public 拍卖查询的端口装配；PostgreSQL 查询、行映射和请求边界由子模块拥有。

use crate::database::DatabaseStore;
use auction_application::auction::{
    ApplicationFuture, AuctionReader, CurrentAuctionSnapshot, PublicAuctionCountdown,
    PublicAuctionListRequest, PublicAuctionPage, PublicRealtimeAuctionStatus,
};
use uuid::Uuid;

mod countdown;
mod mapping;
mod postgres;
mod visibility;

pub(crate) use visibility::{preview_visibility_sql, public_auction_projection_sql};

impl AuctionReader for DatabaseStore {
    fn list_public_auctions(
        &self,
        request: PublicAuctionListRequest,
    ) -> ApplicationFuture<'_, PublicAuctionPage> {
        Box::pin(async move { postgres::list(self, request).await })
    }

    fn current_auction(
        &self,
        world_id: String,
    ) -> ApplicationFuture<'_, Option<CurrentAuctionSnapshot>> {
        Box::pin(async move { postgres::current(self, &world_id).await })
    }

    fn public_auction_countdown(
        &self,
        auction_id: Uuid,
    ) -> ApplicationFuture<'_, Option<PublicAuctionCountdown>> {
        Box::pin(async move { countdown::read(self, auction_id).await })
    }

    fn public_realtime_auction_status(
        &self,
        auction_id: Uuid,
    ) -> ApplicationFuture<'_, Option<PublicRealtimeAuctionStatus>> {
        Box::pin(async move { postgres::realtime_status(self, auction_id).await })
    }
}
