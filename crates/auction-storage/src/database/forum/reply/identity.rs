//! 论坛/MM 共用玩家 ID 的 Users-owner 解析与 forum-only 主体建立。

use std::fmt::Write as _;

use auction_application::{ApplicationError, ForumBidderIdentity, forum_world_name};
use auction_domain::{SettlementWorld, UserId};
use sha2::{Digest, Sha256};
use sqlx::Row;
use uuid::Uuid;

use super::super::super::PostgresStore;
use super::validation::{
    MAX_DISPLAY_NAME_BYTES, MAX_FORUM_PLAYER_ID_BYTES, storage_error, valid_decimal_id, valid_text,
};

/// 按论坛/MM 共用的 `game_player_id` 解析用户；首次论坛竞价可建立 revoked-key 临时主体。
pub(super) async fn resolve_bidder_postgres(
    store: &PostgresStore,
    world: SettlementWorld,
    external_account_id: &str,
    display_name: &str,
) -> Result<ForumBidderIdentity, ApplicationError> {
    if !valid_decimal_id(external_account_id, MAX_FORUM_PLAYER_ID_BYTES)
        || !valid_text(display_name, MAX_DISPLAY_NAME_BYTES)
    {
        return Err(storage_error("论坛/MM 共用玩家身份参数无效"));
    }
    let world_id = forum_world_name(world);
    let mut transaction =
        store.pool().begin().await.map_err(|_| storage_error("开启论坛身份事务失败"))?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")
        .execute(&mut *transaction)
        .await
        .map_err(|_| storage_error("设置论坛身份事务隔离级别失败"))?;
    // Users.game_player_id 跨世界唯一；同一外部 ID 的首次 main/isekai 帖子必须串行决定唯一主体。
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended(lower($1), 0))")
        .bind(external_account_id)
        .execute(&mut *transaction)
        .await
        .map_err(|_| storage_error("锁定论坛玩家身份失败"))?;

    if let Some(row) = sqlx::query(
        "SELECT identity.id, identity.registration_state, identity.status,
                account.id AS user_id, account.status AS user_status,
                account.key_status, account.game_player_id,
                COALESCE(lower(account.game_player_id) = lower(identity.external_account_id), false)
                    AS identity_matches_user
         FROM public.forum_bidder_identities AS identity
         JOIN public.users AS account ON account.id = identity.user_id
         WHERE identity.world_id = $1
           AND identity.provider = 'ehentai'
           AND lower(identity.external_account_id) = lower($2)
         FOR UPDATE OF identity, account",
    )
    .bind(world_id)
    .bind(external_account_id)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|_| storage_error("读取论坛玩家身份失败"))?
    {
        let identity_id: Uuid =
            row.try_get("id").map_err(|_| storage_error("读取论坛身份标识失败"))?;
        let registration_state: String = row
            .try_get("registration_state")
            .map_err(|_| storage_error("读取论坛身份注册状态失败"))?;
        let identity_status: String =
            row.try_get("status").map_err(|_| storage_error("读取论坛身份状态失败"))?;
        let user_id: Uuid =
            row.try_get("user_id").map_err(|_| storage_error("读取论坛用户标识失败"))?;
        let user_status: String =
            row.try_get("user_status").map_err(|_| storage_error("读取论坛用户状态失败"))?;
        let key_status: String =
            row.try_get("key_status").map_err(|_| storage_error("读取论坛用户 key 状态失败"))?;
        let canonical_account_id: Option<String> =
            row.try_get("game_player_id").map_err(|_| storage_error("读取论坛用户玩家标识失败"))?;
        let identity_matches_user: bool = row
            .try_get("identity_matches_user")
            .map_err(|_| storage_error("读取论坛身份一致性失败"))?;
        let canonical_account_id = canonical_account_id
            .ok_or_else(|| storage_error("论坛身份未绑定论坛/MM 共用的玩家标识"))?;
        if identity_id.is_nil()
            || user_id.is_nil()
            || identity_status != "active"
            || user_status != "active"
            || !matches!(key_status.as_str(), "active" | "revoked")
            || !matches!(registration_state.as_str(), "forum_only" | "mm_bound")
            || !identity_matches_user
            || !valid_decimal_id(&canonical_account_id, MAX_FORUM_PLAYER_ID_BYTES)
        {
            return Err(storage_error("论坛身份已被阻止或与用户玩家标识不一致"));
        }
        let next_registration_state =
            if key_status == "active" { "mm_bound" } else { registration_state.as_str() };
        sqlx::query(
            "UPDATE public.forum_bidder_identities
             SET display_name = $1,
                 registration_state = $2,
                 updated_at = transaction_timestamp()
             WHERE id = $3
               AND (display_name IS DISTINCT FROM $1
                    OR registration_state IS DISTINCT FROM $2)",
        )
        .bind(display_name)
        .bind(next_registration_state)
        .bind(identity_id)
        .execute(&mut *transaction)
        .await
        .map_err(|_| storage_error("更新论坛玩家身份失败"))?;
        transaction.commit().await.map_err(|_| storage_error("提交论坛身份事务失败"))?;
        return Ok(ForumBidderIdentity {
            user_id: UserId::from_uuid(user_id),
            external_account_id: canonical_account_id,
            display_name: display_name.to_owned(),
        });
    }

    let user = sqlx::query(
        "SELECT id, status, key_status, game_player_id
         FROM public.users
         WHERE lower(game_player_id) = lower($1)
         FOR UPDATE",
    )
    .bind(external_account_id)
    .fetch_optional(&mut *transaction)
    .await
    .map_err(|_| storage_error("读取论坛/MM 共用玩家身份失败"))?;
    let (user_id, canonical_account_id, registration_state) = if let Some(row) = user {
        let user_id: Uuid = row.try_get("id").map_err(|_| storage_error("读取论坛用户标识失败"))?;
        let user_status: String =
            row.try_get("status").map_err(|_| storage_error("读取论坛用户状态失败"))?;
        let key_status: String =
            row.try_get("key_status").map_err(|_| storage_error("读取论坛用户 key 状态失败"))?;
        let canonical_account_id: Option<String> =
            row.try_get("game_player_id").map_err(|_| storage_error("读取论坛用户玩家标识失败"))?;
        let canonical_account_id = canonical_account_id
            .filter(|value| valid_decimal_id(value, MAX_FORUM_PLAYER_ID_BYTES))
            .ok_or_else(|| storage_error("论坛用户玩家标识无效"))?;
        if user_id.is_nil()
            || user_status != "active"
            || !matches!(key_status.as_str(), "active" | "revoked")
        {
            return Err(storage_error("论坛玩家身份对应用户已被阻止"));
        }
        let registration_state = if key_status == "active" { "mm_bound" } else { "forum_only" };
        (user_id, canonical_account_id, registration_state)
    } else {
        let user_id = Uuid::new_v4();
        let (username, shadow_hash) = shadow_identity(world_id, external_account_id)?;
        sqlx::query(
            "INSERT INTO public.users
                (id, username, status, public_key_hash, game_player_id,
                 key_status, state_version, created_at, updated_at)
             VALUES ($1, $2, 'active', $3, $4, 'revoked', 1,
                     transaction_timestamp(), transaction_timestamp())",
        )
        .bind(user_id)
        .bind(username)
        .bind(shadow_hash)
        .bind(external_account_id)
        .execute(&mut *transaction)
        .await
        .map_err(|_| storage_error("创建论坛临时用户失败"))?;
        (user_id, external_account_id.to_owned(), "forum_only")
    };

    sqlx::query(
        "INSERT INTO public.forum_bidder_identities
            (id, world_id, provider, external_account_id, display_name,
             user_id, registration_state, status, created_at, updated_at)
         VALUES ($1, $2, 'ehentai', $3, $4, $5, $6, 'active',
                 transaction_timestamp(), transaction_timestamp())",
    )
    .bind(Uuid::new_v4())
    .bind(world_id)
    .bind(&canonical_account_id)
    .bind(display_name)
    .bind(user_id)
    .bind(registration_state)
    .execute(&mut *transaction)
    .await
    .map_err(|_| storage_error("保存论坛玩家身份失败"))?;
    transaction.commit().await.map_err(|_| storage_error("提交论坛身份事务失败"))?;
    Ok(ForumBidderIdentity {
        user_id: UserId::from_uuid(user_id),
        external_account_id: canonical_account_id,
        display_name: display_name.to_owned(),
    })
}

fn shadow_identity(
    world_id: &str,
    external_account_id: &str,
) -> Result<(String, Vec<u8>), ApplicationError> {
    let mut hasher = Sha256::new();
    hasher.update(b"auction/forum-shadow/v1\0");
    hasher.update(world_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(external_account_id.as_bytes());
    let digest = hasher.finalize();
    let mut username = String::from("forum-");
    for byte in &digest[..10] {
        write!(&mut username, "{byte:02x}").map_err(|_| storage_error("生成论坛临时用户名失败"))?;
    }
    Ok((username, digest.to_vec()))
}
