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
use auction_domain::EquipmentKeyEnvelope;

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
        "SELECT id, game_player_id, status, state_version
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
    let game_identity: String = row
        .try_get("game_player_id")
        .map_err(|_| ApplicationError::FulfillmentDependencyMismatch)?;
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
    if equipment_id.is_nil() {
        return Err(ApplicationError::FulfillmentDependencyMismatch);
    }

    let row = sqlx::query(
        "SELECT equipment.id AS equipment_id,
                equipment.external_equipment_id,
                equipment_secrets.ciphertext,
                equipment_secrets.nonce,
                equipment_secrets.encryption_key_id,
                equipment_secrets.fingerprint
         FROM public.equipment AS equipment
         INNER JOIN public.equipment_secrets
             ON equipment_secrets.equipment_id = equipment.id
         WHERE equipment.id = $1",
    )
    .bind(equipment_id.as_uuid())
    .fetch_optional(pool)
    .await
    .map_err(|_| ApplicationError::FulfillmentDependencyUnavailable)?
    .ok_or(ApplicationError::FulfillmentEquipmentSecretNotFound)?;

    let stored_equipment_id: Uuid =
        row.try_get("equipment_id").map_err(|_| ApplicationError::FulfillmentDependencyMismatch)?;
    let external_equipment_id: String = row
        .try_get("external_equipment_id")
        .map_err(|_| ApplicationError::FulfillmentDependencyMismatch)?;
    let ciphertext: Vec<u8> =
        row.try_get("ciphertext").map_err(|_| ApplicationError::FulfillmentDependencyMismatch)?;
    let nonce: Vec<u8> =
        row.try_get("nonce").map_err(|_| ApplicationError::FulfillmentDependencyMismatch)?;
    let encryption_key_id: String = row
        .try_get("encryption_key_id")
        .map_err(|_| ApplicationError::FulfillmentDependencyMismatch)?;
    let fingerprint: String =
        row.try_get("fingerprint").map_err(|_| ApplicationError::FulfillmentDependencyMismatch)?;

    if stored_equipment_id != equipment_id.as_uuid()
        || stored_equipment_id.is_nil()
        || !valid_identity(&external_equipment_id)
    {
        return Err(ApplicationError::FulfillmentDependencyMismatch);
    }

    let envelope = EquipmentKeyEnvelope::new(ciphertext, nonce, encryption_key_id, fingerprint)
        .map_err(|_| ApplicationError::FulfillmentDependencyMismatch)?;

    Ok(SettlementFulfillmentEquipmentSecret { equipment_id, external_equipment_id, envelope })
}

#[cfg(feature = "postgres")]
fn valid_identity(value: &str) -> bool {
    !value.is_empty() && value == value.trim() && !value.chars().any(char::is_control)
}

#[cfg(feature = "postgres")]
use sqlx::Row;
