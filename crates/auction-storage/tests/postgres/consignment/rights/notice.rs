//! 权利通知 outbox 的投递存储（7D-5）：结果未知不重发、租约围栏、只向冻结身份派发、outbox 不含明文码。
//!
//! 只写核心规则，每条一正一反；发信本身由 `auction-jobs` 与装配边界的单测用替身覆盖。

use std::time::Duration;

use auction_application::consignment::{
    RightChallengeCrypto, RightNoticeDisposition, RightNoticeStoreError, RightNoticeWorkStore,
    RightsStore,
};
use auction_domain::consignment::rights::format_challenge_code;
use uuid::Uuid;

use super::fixture::{IDENTITY, Rig, challenge_command};

const IP: &str = "198.51.100.9";
const LEASE: Duration = Duration::from_secs(60);

async fn issue(rig: &Rig) -> Uuid {
    let right = rig.seed_right(1, "claim_payment", &["claim_payment"]).await;
    let issued =
        rig.store.challenge(challenge_command(Uuid::new_v4(), 7, IP, &right)).await.expect("发码");
    issued.challenge_id
}

#[tokio::test]
async fn postgres_consignment_notice_unknown_is_final_and_never_claimed_again() {
    let rig = Rig::new().await;
    issue(&rig).await;
    let notices = rig.notice_store();
    let claimed = notices.claim_next(LEASE).await.expect("领取").expect("有一条待发");
    assert!(!claimed.reclaimed);
    notices.complete(claimed.lease, RightNoticeDisposition::Unknown).await.expect("记未知");
    let receipts = "SELECT count(*) FROM public.consignment_commands
                    WHERE operation = 'deliver_right_notice'
                      AND result_body->>'disposition' = 'notice_unknown'";
    assert_eq!(rig.count(receipts).await, 1, "结果与账本回执同事务");
    // 租约早已过期也不得再领：未知永不自动重发。
    rig.advance_minutes(30);
    assert!(notices.claim_next(LEASE).await.expect("领取").is_none());
    assert_eq!(
        rig.count(
            "SELECT count(*) FROM public.consignment_right_notice_results WHERE result = 'unknown'"
        )
        .await,
        1
    );
    // 正例对照：同一结果不能被第二次写入覆盖。
    let second = notices.complete(claimed.lease, RightNoticeDisposition::Sent).await;
    assert_eq!(second.err(), Some(RightNoticeStoreError::LeaseLost));
    let all = "SELECT count(*) FROM public.consignment_commands
               WHERE operation = 'deliver_right_notice'";
    assert_eq!(rig.count(all).await, 1, "第二次完成被拒，不追加回执");
}

#[tokio::test]
async fn postgres_consignment_notice_expired_lease_reclaims_with_new_fence_and_rejects_the_old_holder()
 {
    let rig = Rig::new().await;
    issue(&rig).await;
    let notices = rig.notice_store();
    let first = notices.claim_next(LEASE).await.expect("领取").expect("有一条待发");
    // 租约未过期：另一个 Worker 领不到。
    assert!(notices.claim_next(LEASE).await.expect("领取").is_none());
    rig.advance_minutes(2);
    let second = notices.claim_next(LEASE).await.expect("领取").expect("租约过期后可重新领取");
    assert!(second.lease.claim_fence > first.lease.claim_fence);
    assert!(second.reclaimed, "上一持有者没有留下结果，只能当作可能已发出");
    // 旧围栏的完成与释放都被拒，且不留下结果。
    let stale = notices.complete(first.lease, RightNoticeDisposition::Sent).await;
    assert_eq!(stale.err(), Some(RightNoticeStoreError::LeaseLost));
    assert_eq!(notices.release(first.lease).await.err(), Some(RightNoticeStoreError::LeaseLost));
    assert_eq!(rig.count("SELECT count(*) FROM public.consignment_right_notice_results").await, 0);
    notices.complete(second.lease, RightNoticeDisposition::Unknown).await.expect("新持有者写结果");
}

#[tokio::test]
async fn postgres_consignment_notice_released_work_is_claimed_again_without_reclaim_flag() {
    let rig = Rig::new().await;
    issue(&rig).await;
    let notices = rig.notice_store();
    let first = notices.claim_next(LEASE).await.expect("领取").expect("有一条待发");
    notices.release(first.lease).await.expect("明确未发出时释放");
    let again = notices.claim_next(LEASE).await.expect("领取").expect("释放后可再领");
    assert!(!again.reclaimed, "已释放的工作没有发出过，可以正常发送");
    assert!(again.lease.claim_fence > first.lease.claim_fence);
}

#[tokio::test]
async fn postgres_consignment_notice_dispatches_only_to_the_frozen_identity() {
    let rig = Rig::new().await;
    let challenge_id = issue(&rig).await;
    // 玩家之后改名：outbox 里冻结的身份不变，收件人仍是原名字。
    sqlx::query("UPDATE public.users SET username = 'RenamedFakeSeller' WHERE username = $1")
        .bind(IDENTITY)
        .execute(&rig.pool)
        .await
        .expect("模拟改名");
    let notices = rig.notice_store();
    let claimed = notices.claim_next(LEASE).await.expect("领取").expect("有一条待发");
    assert_eq!(claimed.recipient, IDENTITY);
    notices.complete(claimed.lease, RightNoticeDisposition::Sent).await.expect("记已发");
    // 反例：同一挑战上另有一条指向别的名字的工作（身份与挑战 / 权利冻结身份不符），
    // 只会被抑制，不会派发给那个名字。
    sqlx::query(
        "INSERT INTO public.consignment_right_notice_outbox
            (work_id, notice_kind, challenge_id, right_id, frozen_identity, world_id,
             sealed_payload, nonce, encryption_key_id, created_at)
         SELECT $1, 'locator_guide', challenge_id, right_id, 'ImpostorFakeName', world_id,
                sealed_payload, nonce, encryption_key_id, created_at + interval '1 second'
         FROM public.consignment_right_notice_outbox WHERE challenge_id = $2",
    )
    .bind(Uuid::new_v4())
    .bind(challenge_id)
    .execute(&rig.pool)
    .await
    .expect("植入身份不符的工作");
    assert!(notices.claim_next(LEASE).await.expect("领取").is_none());
    assert_eq!(
        rig.count("SELECT count(*) FROM public.consignment_right_notice_results WHERE result = 'suppressed'")
            .await,
        1
    );
}

#[tokio::test]
async fn postgres_consignment_notice_outbox_holds_no_plaintext_code() {
    let rig = Rig::new().await;
    let challenge_id = issue(&rig).await;
    let code = rig.open_code(challenge_id).await;
    let text = format_challenge_code(&code);
    let (payload, nonce): (Vec<u8>, Vec<u8>) = sqlx::query_as(
        "SELECT sealed_payload, nonce FROM public.consignment_right_notice_outbox
         WHERE challenge_id = $1",
    )
    .bind(challenge_id)
    .fetch_one(&rig.pool)
    .await
    .expect("outbox 行");
    let contains =
        |haystack: &[u8], needle: &[u8]| haystack.windows(needle.len()).any(|w| w == needle);
    assert!(!contains(&payload, code.as_bytes()), "密文里不得出现码的原始字节");
    assert!(!contains(&payload, text.as_str().as_bytes()), "密文里不得出现码的文本");
    assert!(!contains(&nonce, code.as_bytes()));
    // 正例：投递端领到的密文用同一把钥与 AAD 能解封出原码。
    let claimed = rig.notice_store().claim_next(LEASE).await.expect("领取").expect("有一条待发");
    let opened = rig.crypto.open(&claimed.secret.sealed, claimed.secret.aad).expect("解封");
    assert_eq!(opened.as_bytes(), code.as_bytes());
}
