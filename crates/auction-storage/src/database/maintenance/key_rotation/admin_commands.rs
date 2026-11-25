//! Admin 命令响应信封的轮换扫描与 CAS 写回。

use sqlx::{PgPool, Row};
use uuid::Uuid;

use super::KEY_ROTATION_PAGE_SIZE;
use super::KeyEnvelopeSource;
use super::KeyRotationError;
use super::RotatedKeyEnvelope;
use super::StoredKeyEnvelope;
use super::read_failed;
use super::replace_failed;

/// 读取一页仍引用非当前 key ID 的 Admin 命令响应信封；与登录 key 信封共用 `UserLogin` 域。
pub async fn scan_admin_command_key_envelopes(
    pool: &PgPool,
    current_key_id: &str,
    after: Option<Uuid>,
    limit: i64,
) -> Result<Vec<StoredKeyEnvelope>, KeyRotationError> {
    let limit = limit.clamp(1, KEY_ROTATION_PAGE_SIZE);
    let after = after.unwrap_or(Uuid::nil());
    let rows = sqlx::query(
        "SELECT command_id AS entity_id, user_id, NULL::text AS credential_kind,
                NULL::text AS scope, response_ciphertext AS ciphertext,
                response_nonce AS nonce, response_encryption_key_id AS encryption_key_id,
                response_fingerprint AS fingerprint
         FROM public.user_admin_commands
         WHERE response_encryption_key_id IS NOT NULL
           AND response_encryption_key_id <> $1
           AND command_id > $2
         ORDER BY command_id
         LIMIT $3",
    )
    .bind(current_key_id)
    .bind(after)
    .bind(limit)
    .fetch_all(pool)
    .await
    .map_err(read_failed)?;
    rows.into_iter()
        .map(|row| {
            Ok(StoredKeyEnvelope {
                source: KeyEnvelopeSource::AdminCommand,
                equipment_aad: None,
                entity_id: row.try_get("entity_id").map_err(read_failed)?,
                owner_user_id: row.try_get("user_id").map_err(read_failed)?,
                credential_kind: row.try_get("credential_kind").map_err(read_failed)?,
                scope: row.try_get("scope").map_err(read_failed)?,
                generation: None,
                seller_user_id: None,
                purpose: None,
                aad_owner: None,
                challenge_aad: None,
                ciphertext: row.try_get("ciphertext").map_err(read_failed)?,
                nonce: row.try_get("nonce").map_err(read_failed)?,
                encryption_key_id: row.try_get("encryption_key_id").map_err(read_failed)?,
                fingerprint: row.try_get("fingerprint").map_err(read_failed)?,
            })
        })
        .collect()
}

/// 以旧 key ID 为 CAS 条件替换 Admin 命令响应信封。
pub async fn replace_admin_command_key_envelope(
    pool: &PgPool,
    command_id: Uuid,
    expected_key_id: &str,
    rotated: &RotatedKeyEnvelope,
) -> Result<bool, KeyRotationError> {
    let result = sqlx::query(
        "UPDATE public.user_admin_commands
         SET response_ciphertext = $1, response_nonce = $2,
             response_encryption_key_id = $3, response_fingerprint = $4
         WHERE command_id = $5 AND response_encryption_key_id = $6",
    )
    .bind(&rotated.ciphertext)
    .bind(&rotated.nonce)
    .bind(&rotated.encryption_key_id)
    .bind(&rotated.fingerprint)
    .bind(command_id)
    .bind(expected_key_id)
    .execute(pool)
    .await
    .map_err(replace_failed)?;
    Ok(result.rows_affected() == 1)
}
