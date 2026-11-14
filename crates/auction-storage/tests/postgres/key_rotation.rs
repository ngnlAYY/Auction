//! 数据密钥引用盘点：持久化命令响应中的旧密钥也必须阻止移除。
#![cfg(feature = "test-db")]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use auction_domain::DataKeyDomain;
use auction_storage::{
    RotatedKeyEnvelope, replace_admin_command_key_envelope, scan_admin_command_key_envelopes,
    stored_key_ids,
};
use sqlx::PgPool;
use uuid::Uuid;

#[allow(dead_code)]
use crate::support;

#[tokio::test]
async fn postgres_key_reference_inventory_and_rotation_cover_admin_command_responses() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接克隆测试数据库");
    let pool = store.postgres_pool();
    let user_id = Uuid::new_v5(&Uuid::NAMESPACE_OID, b"key-rotation-user");
    let command_id = Uuid::new_v5(&Uuid::NAMESPACE_OID, b"key-rotation-command");
    let key_id = "test-login-key-referenced-only-by-command";
    seed_user(pool, user_id).await;
    seed_command_response(pool, command_id, user_id, key_id).await;

    let stored = stored_key_ids(pool, DataKeyDomain::UserLogin).await.expect("读取密钥引用");
    let count = stored.iter().find(|(stored_key, _)| stored_key == key_id).map(|(_, count)| *count);
    assert_eq!(count, Some(1), "命令响应中的旧密钥必须计入引用");

    let page = scan_admin_command_key_envelopes(pool, "different-key", None, 10)
        .await
        .expect("扫描命令响应信封");
    assert_eq!(page.len(), 1);
    assert_eq!(page[0].owner_user_id, Some(user_id), "命令响应必须携带真实用户身份");
    let rotated = RotatedKeyEnvelope {
        ciphertext: vec![3; 9],
        nonce: vec![4; 12],
        encryption_key_id: "new-login-key".to_owned(),
        fingerprint: "new-fingerprint".to_owned(),
    };
    assert!(
        replace_admin_command_key_envelope(pool, command_id, key_id, &rotated)
            .await
            .expect("替换命令响应信封")
    );
    let remaining = scan_admin_command_key_envelopes(pool, "new-login-key", None, 10)
        .await
        .expect("替换后扫描命令响应信封");
    assert!(remaining.is_empty());

    store.postgres_pool().close().await;
}

async fn seed_user(pool: &PgPool, user_id: Uuid) {
    let mut public_key_hash = [0_u8; 32];
    public_key_hash[..16]
        .copy_from_slice(Uuid::new_v5(&Uuid::NAMESPACE_OID, b"key-hash-a").as_bytes());
    public_key_hash[16..]
        .copy_from_slice(Uuid::new_v5(&Uuid::NAMESPACE_OID, b"key-hash-b").as_bytes());
    sqlx::query("INSERT INTO public.users (id, username, public_key_hash) VALUES ($1, $2, $3)")
        .bind(user_id)
        .bind("key-rotation-user")
        .bind(public_key_hash.as_slice())
        .execute(pool)
        .await
        .expect("写入测试用户");
}

async fn seed_command_response(pool: &PgPool, command_id: Uuid, user_id: Uuid, key_id: &str) {
    sqlx::query(
        "INSERT INTO public.user_admin_commands
            (command_id, command_kind, request_digest, user_id, result_payload,
             response_ciphertext, response_nonce, response_encryption_key_id,
             response_fingerprint)
         VALUES ($1, 'create', $2, $3, '{}'::jsonb, $4, $5, $6, 'fingerprint')",
    )
    .bind(command_id)
    .bind([9_u8; 32].as_slice())
    .bind(user_id)
    .bind([1_u8; 8].as_slice())
    .bind([2_u8; 12].as_slice())
    .bind(key_id)
    .execute(pool)
    .await
    .expect("写入命令响应信封");
}
