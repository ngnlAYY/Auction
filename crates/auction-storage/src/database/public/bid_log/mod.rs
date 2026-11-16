//! 只读已接受的追加式竞价事实；游标、校验与 PostgreSQL 查询分别由子模块拥有。

use auction_application::auction::{PublicBidLogListRequest, PublicBidLogPage, PublicBidLogReader};
use auction_application::core::ApplicationFuture;

use crate::database::DatabaseStore;

mod cursor;
mod postgres;

use cursor::storage_error;
use postgres::list_postgres;

/// 只在 `test-db` 下暴露生产查询原文给 EXPLAIN 夹具，保证测量的是同一份 SQL；默认构建与发布
/// 制品不包含该出口。
#[cfg(feature = "test-db")]
pub mod benchmark_sql {
    pub use super::postgres::page_query_sql;
}

impl PublicBidLogReader for DatabaseStore {
    fn list_public_bid_log(
        &self,
        request: PublicBidLogListRequest,
    ) -> ApplicationFuture<'_, Option<PublicBidLogPage>> {
        Box::pin(async move {
            request.validate().map_err(storage_error)?;
            list_postgres(self, request).await
        })
    }
}
