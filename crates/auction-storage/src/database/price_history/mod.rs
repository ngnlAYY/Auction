//! Public 成交价格历史只读适配器装配。
//!
//! PostgreSQL 与 SQLite Lite 的查询和行映射分别位于后端子模块；共享的游标、筛选、统计
//! 校验和 UUID/时间转换位于 `mapping.rs`，避免把两套 SQL 与公共分页契约混在一起。

use auction_application::{
    ApplicationFuture, PublicPriceHistoryListRequest, PublicPriceHistoryPage,
    PublicPriceHistoryReader,
};

use super::DatabaseStore;

mod mapping;
#[cfg(feature = "postgres")]
mod postgres;
#[cfg(feature = "sqlite")]
mod sqlite;

impl PublicPriceHistoryReader for DatabaseStore {
    fn list_public_price_history(
        &self,
        request: PublicPriceHistoryListRequest,
    ) -> ApplicationFuture<'_, PublicPriceHistoryPage> {
        Box::pin(async move {
            request.validate().map_err(mapping::storage_error)?;
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => postgres::list_postgres(store, request).await,
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(store) => sqlite::list_sqlite(store, request).await,
            }
        })
    }
}
