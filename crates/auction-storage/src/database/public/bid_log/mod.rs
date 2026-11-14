//! 只读已接受的追加式竞价事实；游标、校验与 PostgreSQL 查询分别由子模块拥有。

use auction_application::auction::{
    ApplicationFuture, PublicBidLogListRequest, PublicBidLogPage, PublicBidLogReader,
};

use crate::database::DatabaseStore;

mod common;
mod postgres;

use common::storage_error;
use postgres::list_postgres;

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
