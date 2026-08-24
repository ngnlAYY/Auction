//! SQLite Lite 登录 key 信封读取。

use super::*;

pub(super) async fn load_envelope_sqlite(
    store: &crate::database::SqliteStore,
    user_id: Uuid,
) -> Result<MailLoginKeyEnvelope, ApplicationError> {
    let row = sqlx::query(
        "SELECT ciphertext, nonce, encryption_key_id, fingerprint
         FROM user_login_key_envelopes WHERE user_id = ?",
    )
    .bind(user_id.as_bytes().to_vec())
    .fetch_optional(store.pool())
    .await
    .map_err(|_| repository_error("读取 Lite 登录 key 信封失败"))?
    .ok_or(auction_application::MailProcessingError::LoginKeyNotFound)?;
    let ciphertext: Vec<u8> =
        row.try_get("ciphertext").map_err(|_| repository_error("读取 Lite 登录 key 密文失败"))?;
    let nonce: Vec<u8> =
        row.try_get("nonce").map_err(|_| repository_error("读取 Lite 登录 key nonce 失败"))?;
    let key_id: String = row
        .try_get("encryption_key_id")
        .map_err(|_| repository_error("读取 Lite 登录 key 数据密钥标识失败"))?;
    let fingerprint: String =
        row.try_get("fingerprint").map_err(|_| repository_error("读取 Lite 登录 key 指纹失败"))?;
    MailLoginKeyEnvelope::new(ciphertext, nonce, key_id, fingerprint)
        .map_err(ApplicationError::from)
}
