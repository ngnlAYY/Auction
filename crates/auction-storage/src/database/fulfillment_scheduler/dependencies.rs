//! 结算履约执行前的当前 owner 依赖读取。
//!
//! PostgreSQL 读取只返回应用端口规定的最小快照；任何缺行、数据库错误或损坏字段都必须
//! 以不可用/不匹配错误失败关闭。SQLite Lite 没有这些完整履约依赖，明确返回能力缺失。

use auction_application::{
    ApplicationError, ApplicationFuture, SettlementFulfillmentEquipmentSecret,
    SettlementFulfillmentEquipmentSecretReader, SettlementFulfillmentRecipientReader,
    SettlementFulfillmentRecipientSnapshot,
};
use auction_domain::EquipmentId;
use uuid::Uuid;

use super::DatabaseStore;

#[cfg(feature = "postgres")]
use super::super::equipment_secrets::{
    EquipmentSecretReadError, read_stored_equipment_secret_postgres,
};

impl SettlementFulfillmentRecipientReader for DatabaseStore {
    fn read_recipient(
        &self,
        _user_id: Uuid,
    ) -> ApplicationFuture<'_, SettlementFulfillmentRecipientSnapshot> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => read_recipient_postgres(store.pool(), _user_id).await,
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(ApplicationError::CapabilityUnavailable(
                    "结算履约收件人读取仅支持 PostgreSQL 完整档",
                )),
            }
        })
    }
}

impl SettlementFulfillmentEquipmentSecretReader for DatabaseStore {
    fn read_equipment_secret(
        &self,
        _equipment_id: EquipmentId,
    ) -> ApplicationFuture<'_, SettlementFulfillmentEquipmentSecret> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    read_equipment_secret_postgres(store.pool(), _equipment_id).await
                }
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(ApplicationError::CapabilityUnavailable(
                    "结算履约装备密钥读取仅支持 PostgreSQL 完整档",
                )),
            }
        })
    }
}

#[cfg(feature = "postgres")]
async fn read_recipient_postgres(
    pool: &crate::database::TrackedPool<sqlx::Postgres>,
    user_id: Uuid,
) -> Result<SettlementFulfillmentRecipientSnapshot, ApplicationError> {
    if user_id.is_nil() {
        return Err(ApplicationError::FulfillmentDependencyMismatch);
    }

    let row = sqlx::query(
        "SELECT id, username AS player_name, status, state_version
         FROM public.users
         WHERE id = $1",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .map_err(|_| ApplicationError::FulfillmentDependencyUnavailable)?
    .ok_or(ApplicationError::FulfillmentDependencyUnavailable)?;

    let stored_user_id: Uuid =
        row.try_get("id").map_err(|_| ApplicationError::FulfillmentDependencyMismatch)?;
    let game_identity: String =
        row.try_get("player_name").map_err(|_| ApplicationError::FulfillmentDependencyMismatch)?;
    let status: String =
        row.try_get("status").map_err(|_| ApplicationError::FulfillmentDependencyMismatch)?;
    let state_version: i64 = row
        .try_get("state_version")
        .map_err(|_| ApplicationError::FulfillmentDependencyMismatch)?;

    if stored_user_id != user_id
        || stored_user_id.is_nil()
        || !valid_identity(&game_identity)
        || state_version <= 0
    {
        return Err(ApplicationError::FulfillmentDependencyMismatch);
    }

    let eligible = match status.as_str() {
        "active" => true,
        "blacklisted" => false,
        _ => return Err(ApplicationError::FulfillmentDependencyMismatch),
    };

    Ok(SettlementFulfillmentRecipientSnapshot {
        user_id: stored_user_id,
        game_identity,
        eligible,
        state_version: u64::try_from(state_version)
            .map_err(|_| ApplicationError::FulfillmentDependencyMismatch)?,
    })
}

#[cfg(feature = "postgres")]
async fn read_equipment_secret_postgres(
    pool: &crate::database::TrackedPool<sqlx::Postgres>,
    equipment_id: EquipmentId,
) -> Result<SettlementFulfillmentEquipmentSecret, ApplicationError> {
    let stored = read_stored_equipment_secret_postgres(pool, equipment_id)
        .await
        .map_err(|error| match error {
            EquipmentSecretReadError::Unavailable => {
                ApplicationError::FulfillmentDependencyUnavailable
            }
            EquipmentSecretReadError::Mismatch => ApplicationError::FulfillmentDependencyMismatch,
        })?
        .ok_or(ApplicationError::FulfillmentEquipmentSecretNotFound)?;
    Ok(SettlementFulfillmentEquipmentSecret {
        equipment_id: stored.equipment_id,
        external_equipment_id: stored.external_equipment_id,
        envelope: stored.envelope,
    })
}

#[cfg(feature = "postgres")]
fn valid_identity(value: &str) -> bool {
    !value.is_empty() && value == value.trim() && !value.chars().any(char::is_control)
}

#[cfg(feature = "postgres")]
use sqlx::Row;
