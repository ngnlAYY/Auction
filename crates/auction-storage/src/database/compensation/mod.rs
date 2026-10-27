//! 从 PostgreSQL 结算事实读取补偿池；不能以空集合掩盖读取失败。

#[cfg(feature = "postgres")]
mod postgres;

use auction_application::{
    ApplicationFuture, PublicCompensationListRequest, PublicCompensationMovementPage,
    PublicCompensationMovementRequest, PublicCompensationPage, PublicCompensationReader,
};
use uuid::Uuid;

use super::DatabaseStore;
use crate::database::storage_error;

impl PublicCompensationReader for DatabaseStore {
    fn list_public_compensation(
        &self,
        request: PublicCompensationListRequest,
    ) -> ApplicationFuture<'_, PublicCompensationPage> {
        Box::pin(async move {
            request.validate().map_err(storage_error)?;
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => postgres::list(store, request).await,
            }
        })
    }

    fn list_public_compensation_movements(
        &self,
        auction_id: Uuid,
        request: PublicCompensationMovementRequest,
    ) -> ApplicationFuture<'_, Option<PublicCompensationMovementPage>> {
        Box::pin(async move {
            if auction_id.is_nil() {
                return Err(storage_error("公开补偿账本拍卖标识无效"));
            }
            request.validate().map_err(storage_error)?;
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    postgres::movements(store, auction_id, request).await
                }
            }
        })
    }
}
