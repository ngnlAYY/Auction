//! 凭据域新增预约密文与受保护证据密文的扫描、CAS 重写与旧钥引用统计。

use auction_domain::DataKeyDomain;
use auction_storage::{RotatedKeyEnvelope, scan_key_envelopes, stored_key_ids};

use super::support::{
    ENVELOPE, RESERVATION, USER_1, execute, listing_awaiting_deposit, mail_session,
    reservation_issued, users_and_key,
};
use crate::support::TestDatabase;

const OLD_KEY: &str = "fake-data-key";
const NEW_KEY: &str = "fake-current-key";

fn secret_rows() -> String {
    format!(
        "INSERT INTO public.consignment_reservation_secrets
            (reservation_id, generation, seller_user_id, ciphertext, nonce, encryption_key_id,
             fingerprint, created_at, updated_at, retired_at)
         VALUES ('{RESERVATION}', 1, '{USER_1}', decode(repeat('71', 32), 'hex'),
                 decode(repeat('72', 12), 'hex'), '{OLD_KEY}', repeat('a', 64),
                 '2026-01-01T02:00:00Z', '2026-01-01T02:00:00Z', NULL);
         INSERT INTO public.mail_evidence_envelopes
            (id, purpose, aad_owner, ciphertext, nonce, encryption_key_id, fingerprint,
             created_at, updated_at, retired_at)
         VALUES ('{ENVELOPE}', 'manifest', 'fixture-owner', decode(repeat('73', 32), 'hex'),
                 decode(repeat('74', 12), 'hex'), '{OLD_KEY}', repeat('b', 64),
                 '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', NULL);"
    )
}

fn rotated() -> RotatedKeyEnvelope {
    RotatedKeyEnvelope {
        ciphertext: vec![0x75; 32],
        nonce: vec![0x76; 12],
        encryption_key_id: NEW_KEY.into(),
        fingerprint: "c".repeat(64),
    }
}

/// 新两类密文进入凭据域扫描，重写后旧钥引用归零；回执来源名不携带密钥标识。
#[tokio::test]
async fn postgres_deposit_new_key_envelopes_are_scanned_rewritten_and_counted() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("连接运行账号");
    let pool = store.postgres_pool();
    execute(pool, &users_and_key()).await.expect("写入用户");
    execute(pool, &listing_awaiting_deposit()).await.expect("写入挂单");
    execute(pool, &mail_session()).await.expect("写入邮件会话");
    execute(pool, &reservation_issued()).await.expect("写入预约");
    execute(pool, &secret_rows()).await.expect("写入新密文");

    let page = scan_key_envelopes(pool, DataKeyDomain::Credential, NEW_KEY, None, 200)
        .await
        .expect("扫描凭据域");
    let sources: Vec<_> = page.iter().map(|row| row.source().as_str()).collect();
    assert!(sources.contains(&"credential_keys"));
    assert!(sources.contains(&"consignment_reservation_secrets"));
    assert!(sources.contains(&"mail_evidence_envelopes"));
    assert_eq!(page.len(), 3);

    let counts = stored_key_ids(pool, DataKeyDomain::Credential).await.expect("旧钥引用");
    assert_eq!(counts, vec![(OLD_KEY.to_owned(), 3)]);

    for row in &page {
        let replacement =
            auction_storage::replace_key_envelope(pool, row, &rotated()).await.expect("CAS 重写");
        assert!(replacement.was_replaced(), "{} 应写回", row.source().as_str());
    }
    let counts = stored_key_ids(pool, DataKeyDomain::Credential).await.expect("新钥引用");
    assert_eq!(counts, vec![(NEW_KEY.to_owned(), 3)]);
    let page = scan_key_envelopes(pool, DataKeyDomain::Credential, NEW_KEY, None, 200)
        .await
        .expect("重写后扫描");
    assert!(page.is_empty(), "重写后不应再有非当前钥信封");
}
