//! 从 PostgreSQL 结算事实读取公开履约进度；只取事实，状态判定留在应用层，读取失败不以空页
//! 掩盖。

mod postgres;

use auction_application::core::ApplicationFuture;
use auction_application::public::{
    PublicFulfillmentPage, PublicFulfillmentReader, PublicFulfillmentRequest,
};

use crate::database::DatabaseStore;
use crate::database::storage_error;

impl PublicFulfillmentReader for DatabaseStore {
    fn list_public_fulfillment(
        &self,
        request: PublicFulfillmentRequest,
    ) -> ApplicationFuture<'_, Option<PublicFulfillmentPage>> {
        Box::pin(async move {
            request.validate().map_err(storage_error)?;
            postgres::list(self, request).await
        })
    }
}
