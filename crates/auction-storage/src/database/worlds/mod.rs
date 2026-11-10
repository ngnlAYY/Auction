//! 世界目录查询适配器。

#[cfg(feature = "postgres")]
use crate::database::storage_error_from;

use auction_application::auction::{ApplicationFuture, WorldReader, WorldSnapshot};
use auction_application::core::ApplicationError;
use sqlx::Row;

use super::DatabaseStore;

impl WorldReader for DatabaseStore {
    fn list_worlds(&self) -> ApplicationFuture<'_, Vec<WorldSnapshot>> {
        Box::pin(async move { list_postgres(self).await })
    }
}

#[cfg(feature = "postgres")]
async fn list_postgres(
    store: &super::DatabaseStore,
) -> Result<Vec<WorldSnapshot>, ApplicationError> {
    let rows = sqlx::query(
        "SELECT id, enabled, availability, policy_revision
         FROM public.worlds
         ORDER BY id",
    )
    .fetch_all(store.pool())
    .await
    .map_err(|error| storage_error_from(error, "读取世界目录失败"))?;

    rows.into_iter()
        .map(|row| {
            Ok(WorldSnapshot {
                id: row
                    .try_get("id")
                    .map_err(|error| storage_error_from(error, "读取世界目录失败"))?,
                enabled: row
                    .try_get("enabled")
                    .map_err(|error| storage_error_from(error, "读取世界目录失败"))?,
                availability: row
                    .try_get("availability")
                    .map_err(|error| storage_error_from(error, "读取世界目录失败"))?,
                policy_revision: row
                    .try_get("policy_revision")
                    .map_err(|error| storage_error_from(error, "读取世界目录失败"))?,
            })
        })
        .collect()
}
