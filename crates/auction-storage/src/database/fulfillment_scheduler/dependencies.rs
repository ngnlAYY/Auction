//! 履约调度的权威依赖读取；依赖不可用或不匹配时失败关闭。

use auction_application::auction::ApplicationFuture;
use auction_application::core::ApplicationError;
use auction_application::settlement::{
    SettlementFulfillmentEquipmentSecret, SettlementFulfillmentEquipmentSecretReader,
    SettlementFulfillmentRecipientReader, SettlementFulfillmentRecipientSnapshot,
};
use auction_domain::EquipmentId;
use uuid::Uuid;

use super::DatabaseStore;

use super::super::equipment_secrets::{
    EquipmentSecretReadError, read_stored_equipment_secret_postgres,
};

impl SettlementFulfillmentRecipientReader for DatabaseStore {
    fn read_recipient(
        &self,
        user_id: Uuid,
    ) -> ApplicationFuture<'_, SettlementFulfillmentRecipientSnapshot> {
        Box::pin(async move { read_recipient_postgres(self.pool(), user_id).await })
    }
}

impl SettlementFulfillmentEquipmentSecretReader for DatabaseStore {
    fn read_equipment_secret(
        &self,
        equipment_id: EquipmentId,
    ) -> ApplicationFuture<'_, SettlementFulfillmentEquipmentSecret> {
        Box::pin(async move { read_equipment_secret_postgres(self.pool(), equipment_id).await })
    }
}

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

fn valid_identity(value: &str) -> bool {
    !value.is_empty() && value == value.trim() && !value.chars().any(char::is_control)
}

use sqlx::Row;
