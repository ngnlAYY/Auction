//! 玩家通知冻结收件人的改名退役：除 Admin 改名外，邮件 rebind 与论坛身份同步改名同样使
//! 尚未开始发送的卖方汇总在 sending 围栏前以 `recipient_renamed` 退役。
//!
//! 仅在 `test-db` feature 下编译，入口见 `just test-db`。

#![cfg(feature = "test-db")]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use auction_application::delivery::DeliveryWorkStore;
use chrono::Utc;
use sqlx::PgPool;
use uuid::Uuid;

use crate::support;

async fn seed_user(pool: &PgPool, username: &str) -> Uuid {
    let user_id = Uuid::new_v4();
    let mut public_key_hash = [0_u8; 32];
    public_key_hash[..16].copy_from_slice(user_id.as_bytes());
    public_key_hash[16..].copy_from_slice(Uuid::new_v4().as_bytes());
    sqlx::query("INSERT INTO public.users (id, username, public_key_hash) VALUES ($1, $2, $3)")
        .bind(user_id)
        .bind(username)
        .bind(public_key_hash.as_slice())
        .execute(pool)
        .await
        .expect("写入测试用户");
    user_id
}

/// 邮件 rebind 改名：命令账本与身份历史同事务写入，历史 ID 沿用命令 ID。
async fn seed_rebind_history(pool: &PgPool, user_id: Uuid, previous: &str, next: &str) {
    let command_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO public.user_mail_commands
            (command_id, command_kind, request_hash, user_id, result_code)
         VALUES ($1, 'rebind', decode(repeat('ab', 32), 'hex'), $2, 'rebound')",
    )
    .bind(command_id)
    .bind(user_id)
    .execute(pool)
    .await
    .expect("写入 rebind 命令账本");
    sqlx::query(
        "INSERT INTO public.user_game_identity_history
            (id, user_id, source_command_id, previous_username, next_username,
             previous_game_player_id, next_game_player_id)
         VALUES ($1, $2, $1, $3, $4, $3, $4)",
    )
    .bind(command_id)
    .bind(user_id)
    .bind(previous)
    .bind(next)
    .execute(pool)
    .await
    .expect("写入 rebind 身份历史");
}

async fn seed_forum_history(pool: &PgPool, user_id: Uuid, previous: &str, next: &str) {
    sqlx::query(
        "INSERT INTO public.forum_identity_history
            (id, user_id, world_id, provider, external_account_id,
             previous_username, next_username, previous_forum_account_id, next_forum_account_id)
         VALUES ($1, $2, 'main', 'ehentai', '424242', $3, $4, '424242', '424242')",
    )
    .bind(Uuid::new_v4())
    .bind(user_id)
    .bind(previous)
    .bind(next)
    .execute(pool)
    .await
    .expect("写入论坛身份历史");
}

/// 插入一条冻结收件人的待发送卖方汇总。
async fn insert_summary(pool: &PgPool, recipient: &str) -> Uuid {
    let id = Uuid::new_v4();
    let payload = serde_json::json!({
        "schema_version": 1,
        "operation": "send",
        "world": "main",
        "external_mail_id": id.to_string(),
        "template": "auction_seller_summary",
        "result_code": "summary",
        "user_key_user_id": null,
        "credits": 0,
        "cod": 0,
        "attachment_ids": [],
        "body": "Frozen body",
    });
    sqlx::query(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload, status)
         VALUES ($1, $2, decode(repeat('33', 32), 'hex'), 'seller_summary', $3, $4,
                 $5::jsonb, 'pending')",
    )
    .bind(id)
    .bind(Uuid::new_v4())
    .bind(format!("seller_summary:{id}"))
    .bind(recipient)
    .bind(payload.to_string())
    .execute(pool)
    .await
    .expect("写入待发送卖方汇总");
    id
}

#[tokio::test]
async fn postgres_player_notices_retire_after_rebind_or_forum_rename() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let pool = store.postgres_pool();

    let rebound = seed_user(pool, "RebindNew").await;
    seed_rebind_history(pool, rebound, "RebindOld", "RebindNew").await;
    let synced = seed_user(pool, "ForumNew").await;
    seed_forum_history(pool, synced, "ForumOld", "ForumNew").await;
    // 反例：曾改名又改回原名，当前名仍存在，冻结收件人有效。
    let stable = seed_user(pool, "Stable").await;
    seed_forum_history(pool, stable, "Stable", "StableTemp").await;
    seed_forum_history(pool, stable, "StableTemp", "Stable").await;

    let rebind_notice = insert_summary(pool, "RebindOld").await;
    let forum_notice = insert_summary(pool, "ForumOld").await;
    let stable_notice = insert_summary(pool, "Stable").await;

    store
        .begin_claim_cycle(Utc::now(), std::time::Duration::from_secs(60))
        .await
        .expect("领取前退役清理");

    for delivery_id in [rebind_notice, forum_notice] {
        let (status, failure_code, reason_code): (String, Option<String>, Option<String>) =
            sqlx::query_as(
                "SELECT status, failure_code, result_metadata ->> 'reason_code'
                 FROM public.deliveries WHERE id = $1",
            )
            .bind(delivery_id)
            .fetch_one(pool)
            .await
            .expect("读取退役投递");
        assert_eq!(status, "failed");
        assert_eq!(failure_code.as_deref(), Some("recipient_renamed"));
        assert_eq!(reason_code.as_deref(), Some("recipient_renamed"));
        let dispositions: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM public.delivery_dispositions
             WHERE delivery_id = $1 AND kind = 'preflight_retired'",
        )
        .bind(delivery_id)
        .fetch_one(pool)
        .await
        .expect("读取预检退役处置");
        assert_eq!(dispositions, 1, "退役必须追加一条预检退役处置");
    }
    let status: String = sqlx::query_scalar("SELECT status FROM public.deliveries WHERE id = $1")
        .bind(stable_notice)
        .fetch_one(pool)
        .await
        .expect("读取未改名投递");
    assert_eq!(status, "pending", "当前名存在的收件人不得退役");
    store.postgres_pool().close().await;
}
