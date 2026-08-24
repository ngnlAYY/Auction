//! Public 补偿池读取适配器的后端分派。
//!
//! PostgreSQL 完整档在一个 Serializable 快照中读取结算批次、补偿账本和最新池余额；
//! SQLite Lite 没有结算表，因此显式报告能力缺失，避免把空集合误当成真实补偿池。

#[cfg(feature = "postgres")]
mod postgres;

use auction_application::{
    ApplicationError, ApplicationFuture, PublicCompensationListRequest,
    PublicCompensationMovementPage, PublicCompensationMovementRequest, PublicCompensationPage,
    PublicCompensationReader,
};
use uuid::Uuid;

use super::DatabaseStore;

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
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(_) => Err(ApplicationError::CapabilityUnavailable(
                    "公开补偿池仅支持 PostgreSQL 完整档",
                )),
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
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(_) => Err(ApplicationError::CapabilityUnavailable(
                    "公开补偿账本仅支持 PostgreSQL 完整档",
                )),
            }
        })
    }
}

fn storage_error(message: &str) -> ApplicationError {
    ApplicationError::Repository(auction_application::RepositoryError::Failed(message.to_owned()))
}
