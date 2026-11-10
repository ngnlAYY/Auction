//! 从 PostgreSQL 结算事实与冲正事实读取补偿池；不能以空集合掩盖读取失败。

#[cfg(feature = "postgres")]
mod postgres;

use auction_application::auction::ApplicationFuture;
use auction_application::public::{
    PublicCompensationListRequest, PublicCompensationLotPage, PublicCompensationLotRequest,
    PublicCompensationMovementPage, PublicCompensationMovementRequest, PublicCompensationPage,
    PublicCompensationReader,
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
            postgres::list(self, request).await
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
            postgres::movements(self, auction_id, request).await
        })
    }

    fn list_public_compensation_lots(
        &self,
        auction_id: Uuid,
        request: PublicCompensationLotRequest,
    ) -> ApplicationFuture<'_, Option<PublicCompensationLotPage>> {
        Box::pin(async move {
            if auction_id.is_nil() {
                return Err(storage_error("公开逐件结算拍卖标识无效"));
            }
            request.validate().map_err(storage_error)?;
            postgres::lots(self, auction_id, request).await
        })
    }
}
