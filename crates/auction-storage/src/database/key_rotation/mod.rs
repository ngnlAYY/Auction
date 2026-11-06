//! 数据密钥信封的离线重加密读写面。
//!
//! 只做三件事：按域分页扫描仍引用非当前 key ID 的信封、以 CAS（旧 key ID 必须匹配）替换
//! 信封、列出各域实际出现的 key ID。加解密、指纹重算与回执格式由
//! `auction-integrations`/`auction-cli` 负责；本模块不接触明文、不输出密钥材料。

use auction_domain::DataKeyDomain;
use sqlx::{PgPool, Row};
use thiserror::Error;
use uuid::Uuid;

use super::audit_health::AuditWriteResult as _;

/// 单页扫描上限；离线维护命令逐页推进，避免一次性持有全部密文。
pub const KEY_ROTATION_PAGE_SIZE: i64 = 200;

#[derive(Debug, Error)]
pub enum KeyRotationError {
    #[error("failed to read encrypted key envelopes")]
    Read(#[source] sqlx::Error),
    #[error("failed to replace encrypted key envelope")]
    Replace(#[source] sqlx::Error),
    #[error("failed to append key rotation audit")]
    Audit(#[source] sqlx::Error),
    #[error("unsupported data key domain")]
    UnsupportedDomain,
}

/// 存储中的一封密文信封；`credential_kind`/`scope` 只有凭据域使用。
#[derive(Clone)]
pub struct StoredKeyEnvelope {
    pub entity_id: Uuid,
    pub credential_kind: Option<String>,
    pub scope: Option<String>,
    pub ciphertext: Vec<u8>,
    pub nonce: Vec<u8>,
    pub encryption_key_id: String,
    pub fingerprint: String,
}

impl StoredKeyEnvelope {
    /// 读取后立即清零密文与指纹，避免长期驻留。
    pub fn clear(&mut self) {
        self.ciphertext.fill(0);
        self.fingerprint.clear();
    }
}

impl Drop for StoredKeyEnvelope {
    fn drop(&mut self) {
        self.clear();
    }
}

impl std::fmt::Debug for StoredKeyEnvelope {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StoredKeyEnvelope")
            .field("entity_id", &self.entity_id)
            .field("encryption_key_id", &self.encryption_key_id)
            .finish()
    }
}

/// 重加密后的信封；`ciphertext`/`fingerprint` 由调用方在写入后清理。
pub struct RotatedKeyEnvelope {
    pub ciphertext: Vec<u8>,
    pub nonce: Vec<u8>,
    pub encryption_key_id: String,
    pub fingerprint: String,
}

impl Drop for RotatedKeyEnvelope {
    fn drop(&mut self) {
        self.ciphertext.fill(0);
        self.fingerprint.clear();
    }
}

/// 读取一页仍需重加密的信封；`after` 为上一页最后一个实体 ID（按主键升序）。
pub async fn scan_key_envelopes(
    pool: &PgPool,
    domain: DataKeyDomain,
    current_key_id: &str,
    after: Option<Uuid>,
    limit: i64,
) -> Result<Vec<StoredKeyEnvelope>, KeyRotationError> {
    let limit = limit.clamp(1, KEY_ROTATION_PAGE_SIZE);
    let after = after.unwrap_or(Uuid::nil());
    let rows = match domain {
        DataKeyDomain::Equipment => {
            sqlx::query(
                "SELECT equipment_id AS entity_id, NULL::text AS credential_kind,
                        NULL::text AS scope, ciphertext, nonce, encryption_key_id, fingerprint
                 FROM public.equipment_secrets
                 WHERE encryption_key_id <> $1 AND equipment_id > $2
                 ORDER BY equipment_id
                 LIMIT $3",
            )
            .bind(current_key_id)
            .bind(after)
            .bind(limit)
            .fetch_all(pool)
            .await
        }
        DataKeyDomain::UserLogin => {
            sqlx::query(
                "SELECT user_id AS entity_id, NULL::text AS credential_kind,
                        NULL::text AS scope, ciphertext, nonce, encryption_key_id, fingerprint
                 FROM public.user_login_key_envelopes
                 WHERE encryption_key_id <> $1 AND user_id > $2
                 ORDER BY user_id
                 LIMIT $3",
            )
            .bind(current_key_id)
            .bind(after)
            .bind(limit)
            .fetch_all(pool)
            .await
        }
        DataKeyDomain::Credential => {
            sqlx::query(
                "SELECT id AS entity_id, credential_kind, scope, ciphertext, nonce,
                        encryption_key_id, fingerprint
                 FROM public.credential_keys
                 WHERE encryption_key_id <> $1 AND id > $2
                 ORDER BY id
                 LIMIT $3",
            )
            .bind(current_key_id)
            .bind(after)
            .bind(limit)
            .fetch_all(pool)
            .await
        }
        DataKeyDomain::MailDiagnostics => return Err(KeyRotationError::UnsupportedDomain),
    }
    .map_err(KeyRotationError::Read)?;
    rows.into_iter()
        .map(|row| {
            Ok(StoredKeyEnvelope {
                entity_id: row.try_get("entity_id").map_err(KeyRotationError::Read)?,
                credential_kind: row.try_get("credential_kind").map_err(KeyRotationError::Read)?,
                scope: row.try_get("scope").map_err(KeyRotationError::Read)?,
                ciphertext: row.try_get("ciphertext").map_err(KeyRotationError::Read)?,
                nonce: row.try_get("nonce").map_err(KeyRotationError::Read)?,
                encryption_key_id: row
                    .try_get("encryption_key_id")
                    .map_err(KeyRotationError::Read)?,
                fingerprint: row.try_get("fingerprint").map_err(KeyRotationError::Read)?,
            })
        })
        .collect()
}

/// 以旧 key ID 作为 CAS 条件替换信封；返回是否恰好更新一行。
pub async fn replace_key_envelope(
    pool: &PgPool,
    domain: DataKeyDomain,
    entity_id: Uuid,
    expected_key_id: &str,
    rotated: &RotatedKeyEnvelope,
) -> Result<bool, KeyRotationError> {
    let result = match domain {
        DataKeyDomain::Equipment => {
            sqlx::query(
                "UPDATE public.equipment_secrets
                 SET ciphertext = $1, nonce = $2, encryption_key_id = $3,
                     fingerprint = $4, updated_at = transaction_timestamp()
                 WHERE equipment_id = $5 AND encryption_key_id = $6",
            )
            .bind(&rotated.ciphertext)
            .bind(&rotated.nonce)
            .bind(&rotated.encryption_key_id)
            .bind(&rotated.fingerprint)
            .bind(entity_id)
            .bind(expected_key_id)
            .execute(pool)
            .await
        }
        DataKeyDomain::UserLogin => {
            sqlx::query(
                "UPDATE public.user_login_key_envelopes
                 SET ciphertext = $1, nonce = $2, encryption_key_id = $3,
                     fingerprint = $4, updated_at = transaction_timestamp()
                 WHERE user_id = $5 AND encryption_key_id = $6",
            )
            .bind(&rotated.ciphertext)
            .bind(&rotated.nonce)
            .bind(&rotated.encryption_key_id)
            .bind(&rotated.fingerprint)
            .bind(entity_id)
            .bind(expected_key_id)
            .execute(pool)
            .await
        }
        DataKeyDomain::Credential => {
            sqlx::query(
                "UPDATE public.credential_keys
                 SET ciphertext = $1, nonce = $2, encryption_key_id = $3,
                     fingerprint = $4, updated_at = transaction_timestamp()
                 WHERE id = $5 AND encryption_key_id = $6",
            )
            .bind(&rotated.ciphertext)
            .bind(&rotated.nonce)
            .bind(&rotated.encryption_key_id)
            .bind(&rotated.fingerprint)
            .bind(entity_id)
            .bind(expected_key_id)
            .execute(pool)
            .await
        }
        DataKeyDomain::MailDiagnostics => return Err(KeyRotationError::UnsupportedDomain),
    }
    .map_err(KeyRotationError::Replace)?;
    Ok(result.rows_affected() == 1)
}

/// 列出该域实际出现的 key ID 与信封数量；用于核对旧密钥是否仍被引用。
pub async fn stored_key_ids(
    pool: &PgPool,
    domain: DataKeyDomain,
) -> Result<Vec<(String, i64)>, KeyRotationError> {
    let query = match domain {
        DataKeyDomain::Equipment => {
            "SELECT encryption_key_id, count(*) AS envelopes
             FROM public.equipment_secrets GROUP BY encryption_key_id ORDER BY encryption_key_id"
        }
        DataKeyDomain::UserLogin => {
            "SELECT encryption_key_id, count(*) AS envelopes
             FROM public.user_login_key_envelopes
             GROUP BY encryption_key_id ORDER BY encryption_key_id"
        }
        DataKeyDomain::Credential => {
            "SELECT encryption_key_id, count(*) AS envelopes
             FROM public.credential_keys GROUP BY encryption_key_id ORDER BY encryption_key_id"
        }
        DataKeyDomain::MailDiagnostics => return Err(KeyRotationError::UnsupportedDomain),
    };
    let rows = sqlx::query(query).fetch_all(pool).await.map_err(KeyRotationError::Read)?;
    rows.into_iter()
        .map(|row| {
            let key_id: String =
                row.try_get("encryption_key_id").map_err(KeyRotationError::Read)?;
            let count: i64 = row.try_get("envelopes").map_err(KeyRotationError::Read)?;
            Ok((key_id, count))
        })
        .collect()
}

/// 追加离线重加密审计；metadata 只含 key ID、数量与回执摘要，不含密钥材料。
pub async fn append_reencrypt_audit(
    pool: &PgPool,
    domain: DataKeyDomain,
    actor_id: &str,
    metadata: serde_json::Value,
) -> Result<(), KeyRotationError> {
    sqlx::query(
        "INSERT INTO public.audit_records
            (id, actor_type, actor_id, action, entity_type, entity_id, result, source, metadata)
         VALUES ($1, 'system', $2, 'system.data_key_reencrypt', 'data_key', $3,
                 'success', 'system', $4)",
    )
    .bind(Uuid::new_v4())
    .bind(actor_id)
    .bind(domain.as_str())
    .bind(metadata)
    .execute(pool)
    .await
    .record_audit_failure()
    .map_err(KeyRotationError::Audit)?;
    Ok(())
}
