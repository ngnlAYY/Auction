//! 论坛稳定数字账号与可变 MM 玩家名称的 Users-owner 解析。

use auction_application::{ApplicationError, ForumBidderIdentity, forum_world_name};
use auction_domain::{SettlementWorld, UserId};
use sha2::{Digest, Sha256};
use sqlx::Row;
use uuid::Uuid;

use crate::database::audit_health::AuditWriteResult as _;

use super::super::super::PostgresStore;
use super::validation::{
    MAX_FORUM_ACCOUNT_ID_BYTES, MAX_PLAYER_NAME_BYTES, storage_error, valid_decimal_id, valid_text,
};

/// 按论坛 `showuser` 数字键解析用户；名称变化只更新 MM/展示名，不改变稳定账号归属。
pub(super) async fn resolve_bidder_postgres(
    store: &PostgresStore,
    world: SettlementWorld,
    external_account_id: &str,
    player_name: &str,
) -> Result<ForumBidderIdentity, ApplicationError> {
    if !valid_decimal_id(external_account_id, MAX_FORUM_ACCOUNT_ID_BYTES)
        || !valid_text(player_name, MAX_PLAYER_NAME_BYTES)
    {
        return Err(ApplicationError::ForumIdentityRejected("论坛账号数字 ID 或玩家名称无效"));
    }
    let world_id = forum_world_name(world);
    let mut transaction =
        store.pool().begin().await.map_err(|_| storage_error("开启论坛身份事务失败"))?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")
        .execute(&mut *transaction)
        .await
        .map_err(|_| storage_error("设置论坛身份事务隔离级别失败"))?;
    // 同一 showuser 在 main/isekai 必须串行决定唯一 Users owner。
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(external_account_id)
        .execute(&mut *transaction)
        .await
        .map_err(|_| storage_error("锁定论坛账号数字 ID 失败"))?;

    if let Some(row) = sqlx::query(
        "SELECT identity.id, identity.external_account_id,
                identity.registration_state, identity.status,
                account.id AS user_id, account.status AS user_status,
                account.key_status, account.username, account.forum_account_id
         FROM public.forum_bidder_identities AS identity
         JOIN public.users AS account ON account.id = identity.user_id
         WHERE identity.world_id = $1
           AND identity.provider = 'ehentai'
           AND identity.external_account_id = $2
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
        let canonical_external_account_id: String = row
            .try_get("external_account_id")
            .map_err(|_| storage_error("读取论坛账号数字 ID 失败"))?;
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
        let username: String =
            row.try_get("username").map_err(|_| storage_error("读取论坛玩家名称失败"))?;
        let forum_account_id: Option<String> = row
            .try_get("forum_account_id")
            .map_err(|_| storage_error("读取用户论坛账号 ID 失败"))?;
        if identity_id.is_nil()
            || user_id.is_nil()
            || identity_status != "active"
            || user_status != "active"
            || !matches!(key_status.as_str(), "active" | "revoked")
            || !matches!(registration_state.as_str(), "forum_only" | "mm_bound")
            || canonical_external_account_id != external_account_id
            || forum_account_id.as_deref().is_some_and(|value| value != external_account_id)
            || !valid_text(&username, MAX_PLAYER_NAME_BYTES)
        {
            return Err(ApplicationError::ForumIdentityRejected(
                "论坛身份已被阻止或稳定账号归属不一致",
            ));
        }
        update_user_identity(
            &mut transaction,
            user_id,
            world_id,
            external_account_id,
            player_name,
            &username,
            forum_account_id.as_deref(),
        )
        .await?;
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
        .bind(player_name)
        .bind(next_registration_state)
        .bind(identity_id)
        .execute(&mut *transaction)
        .await
        .map_err(|_| storage_error("更新论坛玩家身份失败"))?;
        transaction.commit().await.map_err(|_| storage_error("提交论坛身份事务失败"))?;
        return Ok(ForumBidderIdentity {
            user_id: UserId::from_uuid(user_id),
            external_account_id: canonical_external_account_id,
            player_name: player_name.to_owned(),
        });
    }

    // 同一数字账号可在两个世界留下映射，但所有映射必须归属同一个用户。
    let linked_rows = sqlx::query(
        "SELECT user_id
         FROM public.forum_bidder_identities
         WHERE provider = 'ehentai' AND external_account_id = $1
         FOR UPDATE",
    )
    .bind(external_account_id)
    .fetch_all(&mut *transaction)
    .await
    .map_err(|_| storage_error("读取论坛账号跨世界归属失败"))?;
    let mut linked_user_id = None;
    for row in linked_rows {
        let user_id: Uuid =
            row.try_get("user_id").map_err(|_| storage_error("读取论坛账号归属用户失败"))?;
        if user_id.is_nil()
            || linked_user_id.is_some_and(|existing_user_id| existing_user_id != user_id)
        {
            return Err(ApplicationError::ForumIdentityRejected("论坛账号跨世界归属不一致"));
        }
        linked_user_id = Some(user_id);
    }

    let rows = sqlx::query(
        "SELECT account.id, account.status, account.key_status, account.username,
                account.forum_account_id,
                EXISTS (
                    SELECT 1
                    FROM public.forum_bidder_identities AS identity
                    WHERE identity.world_id = $3
                      AND identity.provider = 'ehentai'
                      AND identity.user_id = account.id
                ) AS has_world_identity
         FROM public.users AS account
         WHERE ($1::uuid IS NOT NULL AND account.id = $1)
            OR ($1::uuid IS NULL AND (
                account.forum_account_id = $2
                OR (account.forum_account_id IS NULL AND lower(account.username) = lower($4))
            ))
         FOR UPDATE OF account",
    )
    .bind(linked_user_id)
    .bind(external_account_id)
    .bind(world_id)
    .bind(player_name)
    .fetch_all(&mut *transaction)
    .await
    .map_err(|_| storage_error("读取论坛账号对应用户失败"))?;
    if rows.len() > 1 {
        return Err(ApplicationError::ForumIdentityRejected(
            "论坛账号数字 ID 与玩家名称指向不同用户",
        ));
    }

    let (user_id, registration_state) = if let Some(row) = rows.first() {
        let user_id: Uuid = row.try_get("id").map_err(|_| storage_error("读取论坛用户标识失败"))?;
        let user_status: String =
            row.try_get("status").map_err(|_| storage_error("读取论坛用户状态失败"))?;
        let key_status: String =
            row.try_get("key_status").map_err(|_| storage_error("读取论坛用户 key 状态失败"))?;
        let username: String =
            row.try_get("username").map_err(|_| storage_error("读取论坛玩家名称失败"))?;
        let forum_account_id: Option<String> = row
            .try_get("forum_account_id")
            .map_err(|_| storage_error("读取用户论坛账号 ID 失败"))?;
        let has_world_identity: bool = row
            .try_get("has_world_identity")
            .map_err(|_| storage_error("读取论坛用户现有映射失败"))?;
        if user_id.is_nil()
            || user_status != "active"
            || !matches!(key_status.as_str(), "active" | "revoked")
            || !valid_text(&username, MAX_PLAYER_NAME_BYTES)
            || has_world_identity
            || linked_user_id.is_some_and(|existing_user_id| existing_user_id != user_id)
            || forum_account_id.as_deref().is_some_and(|value| value != external_account_id)
            || (key_status == "revoked" && linked_user_id != Some(user_id))
        {
            return Err(ApplicationError::ForumIdentityRejected(
                "论坛玩家对应用户已被阻止或账号归属无法证明",
            ));
        }
        update_user_identity(
            &mut transaction,
            user_id,
            world_id,
            external_account_id,
            player_name,
            &username,
            forum_account_id.as_deref(),
        )
        .await?;
        let registration_state = if key_status == "active" { "mm_bound" } else { "forum_only" };
        (user_id, registration_state)
    } else {
        if linked_user_id.is_some() {
            return Err(ApplicationError::ForumIdentityRejected("论坛账号既有归属用户不存在"));
        }
        let user_id = Uuid::new_v4();
        let placeholder_hash = revoked_key_placeholder_hash(world_id, external_account_id);
        // 论坛证据只建立 revoked 占位 Key；不得自动授予 Public 登录能力。
        sqlx::query(
            "INSERT INTO public.users
                (id, username, status, public_key_hash, game_player_id, forum_account_id,
                 key_status, state_version, created_at, updated_at)
             VALUES ($1, $2, 'active', $3, $2, $4, 'revoked', 1,
                     transaction_timestamp(), transaction_timestamp())",
        )
        .bind(user_id)
        .bind(player_name)
        .bind(placeholder_hash)
        .bind(external_account_id)
        .execute(&mut *transaction)
        .await
        .map_err(|_| storage_error("创建论坛 forum-only 用户失败"))?;
        (user_id, "forum_only")
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
    .bind(external_account_id)
    .bind(player_name)
    .bind(user_id)
    .bind(registration_state)
    .execute(&mut *transaction)
    .await
    .map_err(|_| storage_error("保存论坛玩家身份失败"))?;
    transaction.commit().await.map_err(|_| storage_error("提交论坛身份事务失败"))?;
    Ok(ForumBidderIdentity {
        user_id: UserId::from_uuid(user_id),
        external_account_id: external_account_id.to_owned(),
        player_name: player_name.to_owned(),
    })
}

async fn update_user_identity(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
    world_id: &str,
    forum_account_id: &str,
    player_name: &str,
    previous_username: &str,
    previous_forum_account_id: Option<&str>,
) -> Result<(), ApplicationError> {
    if previous_username == player_name && previous_forum_account_id == Some(forum_account_id) {
        return Ok(());
    }
    let history_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.forum_identity_history
            (id, user_id, world_id, provider, external_account_id,
             previous_username, next_username, previous_forum_account_id,
             next_forum_account_id, created_at)
         VALUES ($1, $2, $3, 'ehentai', $4, $5, $6, $7, $4,
                 transaction_timestamp())",
    )
    .bind(history_id)
    .bind(user_id)
    .bind(world_id)
    .bind(forum_account_id)
    .bind(previous_username)
    .bind(player_name)
    .bind(previous_forum_account_id)
    .execute(&mut **transaction)
    .await
    .map_err(|_| storage_error("追加论坛身份变更历史失败"))?;
    sqlx::query(
        "INSERT INTO public.audit_records
            (id, occurred_at, actor_type, actor_id, action, entity_type, entity_id,
             result, source, metadata)
         VALUES ($1, transaction_timestamp(), 'worker', 'forum-sync',
                 'user.forum_identity_synced', 'user', $2, 'success', 'worker',
                 jsonb_build_object('history_id', $3, 'world_id', $4,
                                    'provider', 'ehentai', 'external_account_id', $5))",
    )
    .bind(Uuid::new_v4())
    .bind(user_id.to_string())
    .bind(history_id)
    .bind(world_id)
    .bind(forum_account_id)
    .execute(&mut **transaction)
    .await
    .record_audit_failure()
    .map_err(|_| storage_error("追加论坛身份审计记录失败"))?;
    // 用户快照时间不倒退；旧 MM 记录可能带有偏快的外部时间，不能让后续论坛绑定破坏投影。
    sqlx::query(
        "UPDATE public.users
         SET username = $1,
             game_player_id = $1,
             forum_account_id = $2,
             state_version = CASE
                 WHEN username IS DISTINCT FROM $1 OR forum_account_id IS DISTINCT FROM $2
                 THEN state_version + 1
                 ELSE state_version
             END,
             updated_at = CASE
                 WHEN username IS DISTINCT FROM $1 OR forum_account_id IS DISTINCT FROM $2
                 THEN GREATEST(created_at, updated_at, transaction_timestamp())
                 ELSE updated_at
             END
         WHERE id = $3",
    )
    .bind(player_name)
    .bind(forum_account_id)
    .bind(user_id)
    .execute(&mut **transaction)
    .await
    .map_err(|_| storage_error("同步论坛账号与玩家名称失败"))?;
    Ok(())
}

fn revoked_key_placeholder_hash(world_id: &str, external_account_id: &str) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(b"auction/forum-revoked-key-placeholder/v1\0");
    hasher.update(world_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(external_account_id.as_bytes());
    hasher.finalize().to_vec()
}
