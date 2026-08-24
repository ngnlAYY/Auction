//! Public 竞价日志只读适配器的模块装配。
//!
//! 竞价日志只读取已经接受的 append-only bid facts；共享 cursor/校验与 PostgreSQL、SQLite
//! 查询分别由子模块拥有，HTTP 层仍只接触应用投影。

use auction_application::{
    ApplicationFuture, PublicBidLogListRequest, PublicBidLogPage, PublicBidLogReader,
};

use super::DatabaseStore;

mod common;
#[cfg(feature = "postgres")]
mod postgres;
#[cfg(feature = "sqlite")]
mod sqlite;

use common::storage_error;

#[cfg(feature = "postgres")]
use postgres::list_postgres;
#[cfg(feature = "sqlite")]
use sqlite::list_sqlite;

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
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(store) => list_sqlite(store, request).await,
            }
        })
    }
}
