//! Public 成交价格历史只读适配器装配。
//!
//! PostgreSQL 完整档从结算事实链读取成交历史；SQLite Lite 没有 settlement owner，因此显式
//! 报告能力缺失，不能只凭可手工写入的 `settled`/`sold` 状态拼装看似完整的成交记录。

use auction_application::{
    ApplicationError, ApplicationFuture, PublicPriceHistoryListRequest, PublicPriceHistoryPage,
    PublicPriceHistoryReader, RepositoryError,
};

use super::DatabaseStore;

#[cfg(feature = "postgres")]
mod mapping;
#[cfg(feature = "postgres")]
mod postgres;

impl PublicPriceHistoryReader for DatabaseStore {
    fn list_public_price_history(
        &self,
        request: PublicPriceHistoryListRequest,
    ) -> ApplicationFuture<'_, PublicPriceHistoryPage> {
        Box::pin(async move {
            request.validate().map_err(|_| {
                ApplicationError::Repository(RepositoryError::Failed(
                    "公开价格历史查询参数无效".to_owned(),
                ))
            })?;
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => postgres::list_postgres(store, request).await,
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(_) => Err(ApplicationError::CapabilityUnavailable(
                    "公开价格历史仅支持 PostgreSQL 完整档",
                )),
            }
        })
    }
}
