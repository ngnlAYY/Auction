//! 只读已接受的追加式竞价事实；游标、校验与 PostgreSQL 查询分别由子模块拥有。

use auction_application::auction::{
    ApplicationFuture, PublicBidLogListRequest, PublicBidLogPage, PublicBidLogReader,
};

use super::DatabaseStore;

mod common;
#[cfg(feature = "postgres")]
mod postgres;

use common::storage_error;

#[cfg(feature = "postgres")]
use postgres::list_postgres;

impl PublicBidLogReader for DatabaseStore {
    fn list_public_bid_log(
        &self,
        request: PublicBidLogListRequest,
    ) -> ApplicationFuture<'_, Option<PublicBidLogPage>> {
        Box::pin(async move {
            request.validate().map_err(storage_error)?;
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => list_postgres(store, request).await,
            }
        })
    }
}
