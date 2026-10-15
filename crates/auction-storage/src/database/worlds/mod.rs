//! 世界目录查询适配器。

use auction_application::{
    ApplicationError, ApplicationFuture, RepositoryError, WorldReader, WorldSnapshot,
};
use sqlx::Row;

use super::DatabaseStore;

impl WorldReader for DatabaseStore {
    fn list_worlds(&self) -> ApplicationFuture<'_, Vec<WorldSnapshot>> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => list_postgres(store).await,
            }
        })
    }
}

#[cfg(feature = "postgres")]
async fn list_postgres(
    store: &super::PostgresStore,
) -> Result<Vec<WorldSnapshot>, ApplicationError> {
    let rows = sqlx::query(
        "SELECT id, enabled, availability, policy_revision
         FROM public.worlds
         ORDER BY id",
    )
    .fetch_all(store.pool())
    .await
    .map_err(|_| storage_error())?;

    rows.into_iter()
        .map(|row| {
            Ok(WorldSnapshot {
                id: row.try_get("id").map_err(|_| storage_error())?,
                enabled: row.try_get("enabled").map_err(|_| storage_error())?,
                availability: row.try_get("availability").map_err(|_| storage_error())?,
                policy_revision: row.try_get("policy_revision").map_err(|_| storage_error())?,
            })
        })
        .collect()
}

fn storage_error() -> ApplicationError {
    ApplicationError::Repository(RepositoryError::Failed("读取世界目录失败".to_owned()))
}
