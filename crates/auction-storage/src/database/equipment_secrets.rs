//! PostgreSQL 装备 key 密文信封的统一只读 owner。
//!
//! Admin/MoogleMail intake 仍在各自权威事务中写入信封；所有运行时读取通过这里共享同一列集合、
//! 结构校验和缺行语义，避免 Public 真实 URL 与 Worker 履约各自复制 secret SQL。

use auction_domain::{EquipmentId, EquipmentKeyEnvelope};
use sqlx::{Row, postgres::PgRow};

use super::TrackedPool;

/// 存储读取失败分类；外层按自己的应用端口映射为安全错误。
pub(super) enum EquipmentSecretReadError {
    Unavailable,
    Mismatch,
}

/// 不含明文 key 的数据库密文投影。
pub(super) struct StoredEquipmentSecret {
    pub equipment_id: EquipmentId,
    pub world_id: String,
    pub external_equipment_id: String,
    pub envelope: EquipmentKeyEnvelope,
}

/// 按装备 owner UUID 读取并校验密文信封；缺行保持为 `None`。
pub(super) async fn read_stored_equipment_secret_postgres(
    pool: &TrackedPool<sqlx::Postgres>,
    equipment_id: EquipmentId,
) -> Result<Option<StoredEquipmentSecret>, EquipmentSecretReadError> {
    let mut secrets = read_stored_equipment_secrets_postgres(pool, &[equipment_id]).await?;
    Ok(secrets.pop())
}

/// 按装备 owner UUID 批量读取密文信封；返回顺序不承诺与输入一致，调用方必须按 owner 重新绑定。
pub(super) async fn read_stored_equipment_secrets_postgres(
    pool: &TrackedPool<sqlx::Postgres>,
    equipment_ids: &[EquipmentId],
) -> Result<Vec<StoredEquipmentSecret>, EquipmentSecretReadError> {
    if equipment_ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut owner_ids = equipment_ids.iter().map(|id| id.as_uuid()).collect::<Vec<_>>();
    if owner_ids.iter().any(uuid::Uuid::is_nil) {
        return Err(EquipmentSecretReadError::Mismatch);
    }
    owner_ids.sort_unstable();
    owner_ids.dedup();

    let rows = sqlx::query(
        "SELECT equipment.id AS equipment_id,
                equipment.world_id,
                equipment.external_equipment_id,
                equipment_secrets.ciphertext,
                equipment_secrets.nonce,
                equipment_secrets.encryption_key_id,
                equipment_secrets.fingerprint
         FROM public.equipment AS equipment
         INNER JOIN public.equipment_secrets
             ON equipment_secrets.equipment_id = equipment.id
         WHERE equipment.id = ANY($1::uuid[])
         ORDER BY equipment.id",
    )
    .bind(owner_ids)
    .fetch_all(pool)
    .await
    .map_err(|_| EquipmentSecretReadError::Unavailable)?;
    rows.into_iter().map(map_stored_equipment_secret).collect()
}

fn map_stored_equipment_secret(
    row: PgRow,
) -> Result<StoredEquipmentSecret, EquipmentSecretReadError> {
    let stored_equipment_id: uuid::Uuid =
        row.try_get("equipment_id").map_err(|_| EquipmentSecretReadError::Mismatch)?;
    let world_id: String =
        row.try_get("world_id").map_err(|_| EquipmentSecretReadError::Mismatch)?;
    let external_equipment_id: String =
        row.try_get("external_equipment_id").map_err(|_| EquipmentSecretReadError::Mismatch)?;
    let ciphertext: Vec<u8> =
        row.try_get("ciphertext").map_err(|_| EquipmentSecretReadError::Mismatch)?;
    let nonce: Vec<u8> = row.try_get("nonce").map_err(|_| EquipmentSecretReadError::Mismatch)?;
    let encryption_key_id: String =
        row.try_get("encryption_key_id").map_err(|_| EquipmentSecretReadError::Mismatch)?;
    let fingerprint: String =
        row.try_get("fingerprint").map_err(|_| EquipmentSecretReadError::Mismatch)?;

    if stored_equipment_id.is_nil()
        || !matches!(world_id.as_str(), "main" | "isekai")
        || external_equipment_id.is_empty()
        || external_equipment_id != external_equipment_id.trim()
        || external_equipment_id.chars().any(char::is_control)
    {
        return Err(EquipmentSecretReadError::Mismatch);
    }
    let envelope = EquipmentKeyEnvelope::new(ciphertext, nonce, encryption_key_id, fingerprint)
        .map_err(|_| EquipmentSecretReadError::Mismatch)?;
    Ok(StoredEquipmentSecret {
        equipment_id: EquipmentId::from_uuid(stored_equipment_id),
        world_id,
        external_equipment_id,
        envelope,
    })
}
