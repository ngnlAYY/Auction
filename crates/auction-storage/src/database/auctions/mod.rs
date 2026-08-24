//! Public 拍卖只读查询的跨后端端口装配。
//!
//! PostgreSQL/SQLite 查询、行映射和请求边界分别由子模块拥有；这里把两套列表示差异收敛到
//! `AuctionReader` 的稳定实现，HTTP 层不直接接触 SQL 行或后端类型。

use super::DatabaseStore;
use auction_application::{
    ApplicationFuture, AuctionReader, CurrentAuctionSnapshot, PublicAuctionListRequest,
    PublicAuctionPage,
};

mod mapping;
#[cfg(feature = "postgres")]
mod postgres;
#[cfg(feature = "sqlite")]
mod sqlite;

impl AuctionReader for DatabaseStore {
    fn list_public_auctions(
        &self,
        request: PublicAuctionListRequest,
    ) -> ApplicationFuture<'_, PublicAuctionPage> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => postgres::list(store, request).await,
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(store) => sqlite::list(store, request).await,
            }
        })
    }

    fn current_auction(
        &self,
        world_id: String,
    ) -> ApplicationFuture<'_, Option<CurrentAuctionSnapshot>> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => postgres::current(store, &world_id).await,
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(store) => sqlite::current(store, &world_id).await,
            }
        })
    }
}
