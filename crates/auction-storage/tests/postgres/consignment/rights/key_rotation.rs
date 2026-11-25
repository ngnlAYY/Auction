//! 权利通知 outbox 密文纳入凭据域密钥轮换：扫描、运行账号 CAS 重封、旧钥引用统计与守卫的受控分支。
//!
//! 只写核心规则：重封后只能用新钥解开且 AAD、其余列不变；旧钥引用归零、重复扫描为空；
//! 无密文的行不计入、不重封；守卫拒绝夹带密文以外列的更新。

use auction_application::consignment::{RightChallengeCrypto, RightsStore, SealedChallengeSecret};
use auction_domain::DataKeyDomain;
use auction_storage::{
    KeyRotationError, RotatedKeyEnvelope, StoredKeyEnvelope, replace_key_envelope,
    scan_key_envelopes, stored_key_ids,
};
use sqlx::PgPool;
use uuid::Uuid;

use super::super::deposit::support::{fails, succeeds};
use super::fixture::{IDENTITY, Rig, TestCrypto, challenge_command};

const OUTBOX: &str = "consignment_right_notice_outbox";
/// 夹具密码适配器写入的旧钥标识。
const OLD_KEY: &str = "fixture-key";
const CURRENT_KEY: &str = "fixture-current";
const IP: &str = "198.51.100.21";

async fn issue(rig: &Rig) -> (Uuid, Uuid) {
    let right = rig.seed_right(1, "claim_payment", &["claim_payment"]).await;
    let issued =
        rig.store.challenge(challenge_command(Uuid::new_v4(), 7, IP, &right)).await.expect("发码");
    (issued.challenge_id, right.right_id)
}

async fn outbox_rows(pool: &PgPool) -> Vec<StoredKeyEnvelope> {
    scan_key_envelopes(pool, DataKeyDomain::Credential, CURRENT_KEY, None, 200)
        .await
        .expect("扫描凭据域")
        .into_iter()
        .filter(|row| row.source().as_str() == OUTBOX)
        .collect()
}

async fn references(pool: &PgPool, key_id: &str) -> i64 {
    stored_key_ids(pool, DataKeyDomain::Credential)
        .await
        .expect("凭据域引用")
        .into_iter()
        .find_map(|(id, count)| (id == key_id).then_some(count))
        .unwrap_or(0)
}

/// 除密文三列外的整行快照。
async fn frozen_columns(pool: &PgPool, challenge_id: Uuid) -> serde_json::Value {
    sqlx::query_scalar(
        "SELECT to_jsonb(outbox) - 'sealed_payload' - 'nonce' - 'encryption_key_id'
         FROM public.consignment_right_notice_outbox AS outbox WHERE challenge_id = $1",
    )
    .bind(challenge_id)
    .fetch_one(pool)
    .await
    .expect("outbox 快照")
}

/// 库里的当前密文，按 `密文 ‖ 指纹` 拆开。
async fn stored_secret(pool: &PgPool, challenge_id: Uuid) -> SealedChallengeSecret {
    let (payload, nonce, key_id): (Vec<u8>, Vec<u8>, String) = sqlx::query_as(
        "SELECT sealed_payload, nonce, encryption_key_id
         FROM public.consignment_right_notice_outbox WHERE challenge_id = $1",
    )
    .bind(challenge_id)
    .fetch_one(pool)
    .await
    .expect("outbox 密文");
    let split = payload.len() - 64;
    SealedChallengeSecret::new(
        payload[..split].to_vec(),
        <[u8; 12]>::try_from(nonce).expect("nonce"),
        key_id,
        String::from_utf8(payload[split..].to_vec()).expect("指纹"),
    )
    .expect("密文形状")
}

#[tokio::test]
async fn postgres_consignment_right_notice_outbox_rekeys_under_the_current_key() {
    let rig = Rig::new().await;
    let (challenge_id, right_id) = issue(&rig).await;
    let code = rig.open_code(challenge_id).await;
    // 无秘密的通知种类三列全空：不计入引用、不进入重封扫描。
    succeeds(
        &rig.pool,
        &format!(
            "INSERT INTO public.{OUTBOX}
                (work_id, notice_kind, right_id, frozen_identity, world_id, created_at)
             VALUES ('{}', 'right_notice', '{right_id}', '{IDENTITY}', 'main',
                     '2026-01-03T01:00:00Z')",
            Uuid::new_v4()
        ),
    )
    .await;
    let before = frozen_columns(&rig.pool, challenge_id).await;
    assert_eq!(references(&rig.pool, OLD_KEY).await, 1, "旧钥只被带密文的一行引用");

    let page = outbox_rows(rig.runtime_pool()).await;
    assert_eq!(page.len(), 1, "只扫描仍带密文的行");
    let row = &page[0];
    let aad = row.challenge_aad.expect("挑战 AAD");
    let sealed = SealedChallengeSecret::new(
        row.ciphertext.clone(),
        <[u8; 12]>::try_from(row.nonce.as_slice()).expect("nonce"),
        row.encryption_key_id.clone(),
        row.fingerprint.clone(),
    )
    .expect("扫描结果形状");
    let opened = rig.crypto.open(&sealed, aad).expect("扫描出的 AAD 与密文可用旧钥解开");
    assert_eq!(opened.as_bytes(), code.as_bytes());

    let current = TestCrypto::new(2);
    let resealed = current.seal(&opened, aad).expect("当前钥重封");
    let rotated = RotatedKeyEnvelope {
        ciphertext: resealed.ciphertext().to_vec(),
        nonce: resealed.nonce().to_vec(),
        encryption_key_id: CURRENT_KEY.into(),
        fingerprint: resealed.fingerprint().to_owned(),
    };
    let replaced =
        replace_key_envelope(rig.runtime_pool(), row, &rotated).await.expect("运行账号 CAS 写回");
    assert!(replaced.was_replaced());

    let stored = stored_secret(&rig.pool, challenge_id).await;
    assert_eq!(stored.encryption_key_id(), CURRENT_KEY);
    assert_eq!(current.open(&stored, aad).expect("新钥按原 AAD 解开").as_bytes(), code.as_bytes());
    assert!(rig.crypto.open(&stored, aad).is_err(), "旧钥不得再解开");
    assert_eq!(frozen_columns(&rig.pool, challenge_id).await, before, "密文以外的列逐字不变");
    assert_eq!(references(&rig.pool, OLD_KEY).await, 0);
    assert_eq!(references(&rig.pool, CURRENT_KEY).await, 1);
    // 重复执行：不再扫到该行（rewritten 为 0）；旧扫描结果的 CAS 中止，不覆盖新密文。
    assert!(outbox_rows(rig.runtime_pool()).await.is_empty());
    assert!(matches!(
        replace_key_envelope(rig.runtime_pool(), row, &rotated).await,
        Err(KeyRotationError::Conflict)
    ));
}

#[tokio::test]
async fn postgres_consignment_right_notice_outbox_guard_only_allows_a_pure_rekey() {
    let rig = Rig::new().await;
    let (challenge_id, _) = issue(&rig).await;
    let rekey = format!(
        "sealed_payload = decode(repeat('5a', octet_length(sealed_payload)), 'hex'),
         nonce = decode(repeat('5b', 12), 'hex'), encryption_key_id = '{CURRENT_KEY}'"
    );
    // 反例：换密文的同时改身份或租约列，即使由维护账号执行也被守卫拒绝。
    for extra in ["frozen_identity = 'OtherFakeName'", "claim_fence = claim_fence + 1"] {
        fails(
            &rig.pool,
            &format!(
                "UPDATE public.{OUTBOX} SET {rekey}, {extra} WHERE challenge_id = '{challenge_id}'"
            ),
            "23514",
            Some("c7_notice_immutable"),
        )
        .await;
    }
    // 反例：运行账号只改载荷而不换钥（任意覆写）同样被拒。
    fails(
        rig.runtime_pool(),
        &format!(
            "UPDATE public.{OUTBOX} SET sealed_payload = decode('00', 'hex')
             WHERE challenge_id = '{challenge_id}'"
        ),
        "23514",
        Some("c7_notice_immutable"),
    )
    .await;
    // 正例：只换密文三列由运行账号提交。
    succeeds(
        rig.runtime_pool(),
        &format!("UPDATE public.{OUTBOX} SET {rekey} WHERE challenge_id = '{challenge_id}'"),
    )
    .await;
    assert_eq!(references(&rig.pool, CURRENT_KEY).await, 1);
}
