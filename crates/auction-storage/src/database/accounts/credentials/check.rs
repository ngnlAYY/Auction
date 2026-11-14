//! 凭据校验专用只读信封查询；不取得 Worker 写入所有权，不开启跨网络事务。

use auction_application::admin::{AdminCredentialCheckRecord, AdminCredentialCheckStore};
use auction_application::auction::ApplicationFuture;
use uuid::Uuid;

use super::mapping::map_postgres_credential;
use super::mapping::map_storage_error;
use super::{AdminCredentialError, DatabaseStore, admin_error};

impl AdminCredentialCheckStore for DatabaseStore {
    fn load_for_check(
        &self,
        credential_id: Uuid,
    ) -> ApplicationFuture<'_, AdminCredentialCheckRecord> {
        Box::pin(async move {
            if credential_id.is_nil() {
                return Err(admin_error(AdminCredentialError::InvalidRequest));
            }
            {
                use super::AdminCredentialEnvelope;
                use sqlx::Row;
                let row = sqlx::query(
                    "SELECT id, credential_kind, scope, label, player_name, status, state_version,
                                created_at, rotated_at, updated_at, ciphertext, nonce,
                                encryption_key_id, fingerprint
                         FROM public.credential_keys WHERE id = $1",
                )
                .bind(credential_id)
                .fetch_optional(self.pool())
                .await
                .map_err(map_storage_error)?
                .ok_or_else(|| admin_error(AdminCredentialError::NotFound))?;
                Ok(AdminCredentialCheckRecord {
                    credential: map_postgres_credential(&row)?,
                    envelope: AdminCredentialEnvelope::new(
                        row.try_get("ciphertext").map_err(map_storage_error)?,
                        row.try_get("nonce").map_err(map_storage_error)?,
                        row.try_get("encryption_key_id").map_err(map_storage_error)?,
                        row.try_get("fingerprint").map_err(map_storage_error)?,
                    )
                    .map_err(admin_error)?,
                })
            }
        })
    }
}
