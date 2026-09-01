//! users owner 的 Public 登录投影。
//!
//! 这里只按公开登录 key 的摘要读取最小身份，不把用户记录、认证材料或管理字段越过应用端口。

use auction_application::{
    ApplicationError, ApplicationFuture, PublicLoginAuthenticator, PublicLoginIdentity,
    RepositoryError, SessionError,
};
use sha2::{Digest, Sha256};
use sqlx::Row;
#[cfg(feature = "sqlite")]
use uuid::Uuid;

use super::DatabaseStore;

mod admin;
mod mail;

impl PublicLoginAuthenticator for DatabaseStore {
    fn authenticate(&self, key: Vec<u8>) -> ApplicationFuture<'_, PublicLoginIdentity> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => authenticate_postgres(store, &key).await,
                #[cfg(feature = "sqlite")]
                DatabaseStore::Sqlite(store) => authenticate_sqlite(store, &key).await,
            }
        })
    }
}

#[cfg(feature = "postgres")]
async fn authenticate_postgres(
    store: &super::PostgresStore,
    key: &[u8],
) -> Result<PublicLoginIdentity, ApplicationError> {
    let key_hash = Sha256::digest(key);
    let row = sqlx::query(
        "SELECT account.id, account.username, account.game_player_id, account.status, account.key_status,
                blacklist.public_reason
         FROM public.users AS account
         LEFT JOIN public.user_blacklist AS blacklist
           ON blacklist.user_id = account.id AND blacklist.revoked_at IS NULL
         WHERE account.public_key_hash = $1",
    )
    .bind(key_hash.as_slice())
    .fetch_optional(store.pool())
    .await
    .map_err(|_| storage_error("读取 Public 登录身份失败"))?;

    let Some(row) = row else {
        return Err(SessionError::InvalidCredentials.into());
    };
    let key_status: String =
        row.try_get("key_status").map_err(|_| storage_error("读取登录密钥状态失败"))?;
    if key_status != "active" {
        return Err(SessionError::InvalidCredentials.into());
    }
    let status: String = row.try_get("status").map_err(|_| storage_error("读取用户状态失败"))?;
    let active = match status.as_str() {
        "active" => true,
        "blacklisted" => false,
        _ => return Err(storage_error("用户状态不在允许集合内")),
    };
    Ok(PublicLoginIdentity {
        user_id: row.try_get("id").map_err(|_| storage_error("读取用户标识失败"))?,
        username: row.try_get("username").map_err(|_| storage_error("读取用户名失败"))?,
        game_player_id: row
            .try_get("game_player_id")
            .map_err(|_| storage_error("读取游戏玩家标识失败"))?,
        active,
        public_reason: row
            .try_get("public_reason")
            .map_err(|_| storage_error("读取公开封禁原因失败"))?,
    })
}

#[cfg(feature = "sqlite")]
async fn authenticate_sqlite(
    store: &super::SqliteStore,
    key: &[u8],
) -> Result<PublicLoginIdentity, ApplicationError> {
    let key_hash = Sha256::digest(key);
    let row = sqlx::query(
        "SELECT account.id, account.username, account.game_player_id, account.status, account.key_status,
                blacklist.public_reason
         FROM users AS account
         LEFT JOIN user_blacklist AS blacklist
           ON blacklist.user_id = account.id AND blacklist.revoked_at IS NULL
         WHERE account.public_key_hash = ?",
    )
    .bind(key_hash.to_vec())
    .fetch_optional(store.pool())
    .await
    .map_err(|_| storage_error("读取 Lite Public 登录身份失败"))?;

    let Some(row) = row else {
        return Err(SessionError::InvalidCredentials.into());
    };
    let key_status: String =
        row.try_get("key_status").map_err(|_| storage_error("读取 Lite 登录密钥状态失败"))?;
    if key_status != "active" {
        return Err(SessionError::InvalidCredentials.into());
    }
    let status: String =
        row.try_get("status").map_err(|_| storage_error("读取 Lite 用户状态失败"))?;
    let active = match status.as_str() {
        "active" => true,
        "blacklisted" => false,
        _ => return Err(storage_error("Lite 用户状态不在允许集合内")),
    };
    let id = Uuid::from_slice(
        &row.try_get::<Vec<u8>, _>("id").map_err(|_| storage_error("读取 Lite 用户标识失败"))?,
    )
    .map_err(|_| storage_error("Lite 用户标识格式无效"))?;
    Ok(PublicLoginIdentity {
        user_id: id,
        username: row.try_get("username").map_err(|_| storage_error("读取 Lite 用户名失败"))?,
        game_player_id: row
            .try_get("game_player_id")
            .map_err(|_| storage_error("读取 Lite 游戏玩家标识失败"))?,
        active,
        public_reason: row
            .try_get("public_reason")
            .map_err(|_| storage_error("读取 Lite 公开封禁原因失败"))?,
    })
}

fn storage_error(message: &str) -> ApplicationError {
    ApplicationError::Repository(RepositoryError::Failed(message.to_owned()))
}
