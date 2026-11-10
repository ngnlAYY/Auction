//! users owner 唯一的登录 key 信封写入；Admin 签发与 MoogleMail 注册/重绑共用。

use auction_application::mail::MailLoginKeyEnvelope;
use chrono::{DateTime, Utc};
use uuid::Uuid;

/// 在调用方事务内覆盖用户当前信封；`updated_at` 只前进，不因时钟回拨倒退。
pub(super) async fn upsert_login_key_envelope(
    transaction: &mut crate::database::TrackedTransaction<'_, sqlx::Postgres>,
    user_id: Uuid,
    envelope: &MailLoginKeyEnvelope,
    now: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO public.user_login_key_envelopes
            (user_id, ciphertext, nonce, encryption_key_id, fingerprint, created_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $6)
         ON CONFLICT (user_id) DO UPDATE
         SET ciphertext = EXCLUDED.ciphertext,
             nonce = EXCLUDED.nonce,
             encryption_key_id = EXCLUDED.encryption_key_id,
             fingerprint = EXCLUDED.fingerprint,
             updated_at = GREATEST(user_login_key_envelopes.created_at,
                                   user_login_key_envelopes.updated_at, EXCLUDED.updated_at)",
    )
    .bind(user_id)
    .bind(envelope.ciphertext())
    .bind(envelope.nonce())
    .bind(envelope.encryption_key_id())
    .bind(envelope.fingerprint())
    .bind(now)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}
