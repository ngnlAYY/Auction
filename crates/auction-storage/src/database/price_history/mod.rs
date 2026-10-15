//! 从 PostgreSQL 结算事实链读取成交价格历史，不以当前状态重算已结算结果。

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
            }
        })
    }
}
