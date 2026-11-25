//! 挑战：5 次失败上限、每身份 10 分钟一封（并发下仍一封）、浏览器绑定、真实与虚拟同一段写路径、
//! 密钥轮换后失败关闭。

use std::collections::BTreeSet;

use auction_application::consignment::{RightNoticeKind, RightsStore, RightsStoreError};
use auction_domain::consignment::rights::ChallengeCode;
use uuid::Uuid;

use super::fixture::{IDENTITY, Rig, challenge_command, locate_command, verify_command};

const IP: &str = "198.51.100.7";

fn wrong_code() -> ChallengeCode {
    ChallengeCode::from_entropy([0xee; 16])
}

#[tokio::test]
async fn postgres_consignment_rights_fifth_failure_closes_the_challenge_even_for_the_right_code() {
    let rig = Rig::new().await;
    let right = rig.seed_right(1, "claim_payment", &["claim_payment"]).await;
    let issued =
        rig.store.challenge(challenge_command(Uuid::new_v4(), 7, IP, &right)).await.expect("发码");
    let code = rig.open_code(issued.challenge_id).await;
    // 反例：连续 5 次错码，每次统一 challenge_invalid；第 6 次即使码对也拒，且没有会话。
    for _ in 0..5 {
        let result = rig
            .store
            .verify(verify_command(Uuid::new_v4(), 7, issued.challenge_id, wrong_code()))
            .await;
        assert_eq!(result.err(), Some(RightsStoreError::ChallengeInvalid));
    }
    let sixth = rig
        .store
        .verify(verify_command(Uuid::new_v4(), 7, issued.challenge_id, code.clone()))
        .await;
    assert_eq!(sixth.err(), Some(RightsStoreError::ChallengeInvalid));
    assert_eq!(
        rig.count("SELECT count(*) FROM public.consignment_right_challenge_attempts WHERE outcome = 'failed'")
            .await,
        5
    );
    assert_eq!(rig.count("SELECT count(*) FROM public.consignment_right_sessions").await, 0);
    // 正例：换一个新挑战（窗口过后），正确的码一次通过并只消费一次。
    rig.advance_minutes(11);
    let fresh = rig
        .store
        .challenge(challenge_command(Uuid::new_v4(), 7, IP, &right))
        .await
        .expect("再次发码");
    let fresh_code = rig.open_code(fresh.challenge_id).await;
    let verified = rig
        .store
        .verify(verify_command(Uuid::new_v4(), 7, fresh.challenge_id, fresh_code.clone()))
        .await
        .expect("正确的码通过");
    assert_eq!(verified.right_id, right.right_id);
    let again =
        rig.store.verify(verify_command(Uuid::new_v4(), 7, fresh.challenge_id, fresh_code)).await;
    assert_eq!(again.err(), Some(RightsStoreError::ChallengeInvalid), "挑战至多消费一次");
}

#[tokio::test]
async fn postgres_consignment_rights_concurrent_requests_write_one_notice_per_identity_window() {
    let rig = Rig::new().await;
    let right = rig.seed_right(1, "claim_payment", &["claim_payment"]).await;
    let mut tasks = Vec::new();
    for index in 0..8_u8 {
        let store = rig.store.clone();
        let command = challenge_command(Uuid::new_v4(), 20 + index, IP, &right);
        tasks.push(tokio::spawn(async move { store.challenge(command).await }));
    }
    let mut ids = BTreeSet::new();
    for task in tasks {
        // 被节流的请求与发信的请求同形：都成功、都有自己的挑战 ID。
        ids.insert(task.await.expect("任务").expect("受理").challenge_id);
    }
    assert_eq!(ids.len(), 8);
    assert_eq!(
        rig.count("SELECT count(*) FROM public.consignment_right_notice_outbox").await,
        1,
        "同一冻结身份窗口内并发下仍只有一封"
    );
    // 正例：窗口过后可以再发一封。
    rig.advance_minutes(11);
    rig.store
        .challenge(challenge_command(Uuid::new_v4(), 40, IP, &right))
        .await
        .expect("窗口后受理");
    assert_eq!(rig.count("SELECT count(*) FROM public.consignment_right_notice_outbox").await, 2);
}

#[tokio::test]
async fn postgres_consignment_rights_other_browser_is_rejected_without_burning_attempts() {
    let rig = Rig::new().await;
    let right = rig.seed_right(1, "claim_payment", &["claim_payment"]).await;
    let issued =
        rig.store.challenge(challenge_command(Uuid::new_v4(), 7, IP, &right)).await.expect("发码");
    let code = rig.open_code(issued.challenge_id).await;
    // 反例：另一个浏览器带着正确的码也被拒，且不消耗原浏览器的失败次数。
    let foreign = rig
        .store
        .verify(verify_command(Uuid::new_v4(), 9, issued.challenge_id, code.clone()))
        .await;
    assert_eq!(foreign.err(), Some(RightsStoreError::ChallengeInvalid));
    assert_eq!(
        rig.count("SELECT count(*) FROM public.consignment_right_challenge_attempts").await,
        0
    );
    // 正例：原浏览器随后仍能通过。
    rig.store
        .verify(verify_command(Uuid::new_v4(), 7, issued.challenge_id, code))
        .await
        .expect("原浏览器通过");
}

#[tokio::test]
async fn postgres_consignment_rights_real_virtual_and_unmatched_targets_share_one_write_path() {
    let rig = Rig::new().await;
    let right = rig.seed_right(1, "claim_payment", &["claim_payment"]).await;
    let unknown = rig.seed_right(2, "claim_equipment", &[]).await;
    // 真实身份与虚拟身份、已知与未知定位符，各走一次。
    let real_locate = rig
        .store
        .locate(locate_command(Uuid::new_v4(), 1, "198.51.100.1", IDENTITY))
        .await
        .expect("找回");
    let virtual_locate = rig
        .store
        .locate(locate_command(Uuid::new_v4(), 2, "198.51.100.2", "NoSuchPlayer"))
        .await
        .expect("虚拟找回");
    let real_challenge = rig
        .store
        .challenge(challenge_command(Uuid::new_v4(), 3, "198.51.100.3", &right))
        .await
        .expect("发码");
    let mut missing = challenge_command(Uuid::new_v4(), 4, "198.51.100.4", &unknown);
    missing.locator_digest = auction_domain::consignment::rights::locator_digest(
        &auction_domain::consignment::rights::RightLocator::from_entropy([0x99; 32]),
    );
    let virtual_challenge = rig.store.challenge(missing).await.expect("未匹配定位符");
    // 同形：四次都有账本回执与挑战行，回执字段集合相同。
    assert_eq!(
        rig.count("SELECT count(*) FROM public.consignment_commands WHERE operation IN ('locate_right', 'challenge_right')")
            .await,
        4
    );
    assert_eq!(rig.count("SELECT count(*) FROM public.consignment_right_challenges").await, 4);
    let shapes: Vec<String> = sqlx::query_scalar(
        "SELECT (SELECT string_agg(key, ',' ORDER BY key) FROM jsonb_object_keys(result_body - 'command_id' - 'operation') AS key)
         FROM public.consignment_commands WHERE operation IN ('locate_right', 'challenge_right')",
    )
    .fetch_all(&rig.pool)
    .await
    .expect("回执形状");
    assert_eq!(shapes.iter().collect::<BTreeSet<_>>().len(), 1, "{shapes:?}");
    // 差别只在 outbox：只有真实且未被节流的目标写行。第一封（找回）占用了该身份的窗口，
    // 紧随其后的真实发码被节流，所以只有 1 行，且来自真实找回。
    let notices: Vec<Uuid> =
        sqlx::query_scalar("SELECT challenge_id FROM public.consignment_right_notice_outbox")
            .fetch_all(&rig.pool)
            .await
            .expect("outbox");
    assert_eq!(notices, vec![real_locate.challenge_id]);
    for virtual_id in
        [virtual_locate.challenge_id, virtual_challenge.challenge_id, real_challenge.challenge_id]
    {
        assert_eq!(
            rig.count(&format!(
                "SELECT count(*) FROM public.consignment_right_challenges WHERE challenge_id = '{virtual_id}' AND is_virtual"
            ))
            .await,
            1
        );
    }
}

#[tokio::test]
async fn postgres_consignment_rights_rotated_key_fails_closed_and_missing_crypto_is_unavailable() {
    let rig = Rig::new().await;
    let right = rig.seed_right(1, "claim_payment", &["claim_payment"]).await;
    let issued =
        rig.store.challenge(challenge_command(Uuid::new_v4(), 7, IP, &right)).await.expect("发码");
    let code = rig.open_code(issued.challenge_id).await;
    // 反例：数据钥轮换后，进行中的挑战即使码正确也通不过（需重新申请）。
    let rotated = rig.store_with(Some(super::fixture::TestCrypto::new(2)));
    let result =
        rotated.verify(verify_command(Uuid::new_v4(), 7, issued.challenge_id, code.clone())).await;
    assert_eq!(result.err(), Some(RightsStoreError::ChallengeInvalid));
    // 反例：未注入密码端口时失败关闭，且不写任何行。
    let before = rig.count("SELECT count(*) FROM public.consignment_commands").await;
    let missing = rig.store_with(None);
    let result = missing.challenge(challenge_command(Uuid::new_v4(), 8, IP, &right)).await;
    assert_eq!(result.err(), Some(RightsStoreError::Unavailable));
    assert_eq!(rig.count("SELECT count(*) FROM public.consignment_commands").await, before);
    // 正例：原钥仍可通过（只有 5 次上限中的 1 次被上面的轮换请求占用）。
    rig.store
        .verify(verify_command(Uuid::new_v4(), 7, issued.challenge_id, code))
        .await
        .expect("原钥通过");
}

#[tokio::test]
async fn postgres_consignment_rights_verify_replay_returns_receipt_but_fails_closed_after_rotation()
{
    let rig = Rig::new().await;
    let right = rig.seed_right(1, "claim_payment", &["claim_payment"]).await;
    let issued =
        rig.store.challenge(challenge_command(Uuid::new_v4(), 7, IP, &right)).await.expect("发码");
    let code = rig.open_code(issued.challenge_id).await;
    let command_id = Uuid::new_v4();
    let first = rig
        .store
        .verify(verify_command(command_id, 7, issued.challenge_id, code.clone()))
        .await
        .expect("首次核验");
    // 正例：未轮换时同 ID 重放回原回执（同一权利与版本、原截止），只换令牌。
    let replay = rig
        .store
        .verify(verify_command(command_id, 7, issued.challenge_id, code.clone()))
        .await
        .expect("同 ID 重放");
    assert_eq!(replay.right_id, first.right_id);
    assert_eq!(replay.right_version, first.right_version);
    assert_eq!(replay.expires_at, first.expires_at);
    // 反例：数据钥轮换后同 ID 重放失败关闭，不换发令牌。
    let tokens = "SELECT count(*) FROM public.consignment_right_session_tokens";
    let before = rig.count(tokens).await;
    let rotated = rig.store_with(Some(super::fixture::TestCrypto::new(2)));
    let result =
        rotated.verify(verify_command(command_id, 7, issued.challenge_id, code.clone())).await;
    assert_eq!(result.err(), Some(RightsStoreError::ChallengeInvalid));
    assert_eq!(rig.count(tokens).await, before, "被拒的重放不得换发令牌");
    // 反例：同 ID 重放带错码（请求哈希相同、码不同）也不换发令牌。
    let wrong =
        rig.store.verify(verify_command(command_id, 7, issued.challenge_id, wrong_code())).await;
    assert_eq!(wrong.err(), Some(RightsStoreError::ChallengeInvalid));
    assert_eq!(rig.count(tokens).await, before);
}

#[tokio::test]
async fn postgres_consignment_rights_virtual_target_replay_keeps_the_challenge_id() {
    let rig = Rig::new().await;
    let command_id = Uuid::new_v4();
    let first = rig
        .store
        .locate(locate_command(command_id, 2, "198.51.100.2", "NoSuchPlayer"))
        .await
        .expect("虚拟找回");
    let replay = rig
        .store
        .locate(locate_command(command_id, 2, "198.51.100.2", "NoSuchPlayer"))
        .await
        .expect("虚拟找回重放");
    // 虚拟路径同样落账：同命令重放返回同一个 challenge_id 与截止，且不多写挑战行。
    assert_eq!(replay.challenge_id, first.challenge_id);
    assert_eq!(replay.expires_at, first.expires_at);
    assert_eq!(rig.count("SELECT count(*) FROM public.consignment_right_challenges").await, 1);
    // 与真实目标响应同形：回执键集合一致。
    let right = rig.seed_right(1, "claim_payment", &["claim_payment"]).await;
    rig.store
        .challenge(challenge_command(Uuid::new_v4(), 3, "198.51.100.3", &right))
        .await
        .expect("真实");
    let shapes: Vec<String> = sqlx::query_scalar(
        "SELECT (SELECT string_agg(key, ',' ORDER BY key) FROM jsonb_object_keys(result_body - 'command_id' - 'operation') AS key)
         FROM public.consignment_commands WHERE operation IN ('locate_right', 'challenge_right')",
    )
    .fetch_all(&rig.pool)
    .await
    .expect("回执形状");
    assert_eq!(shapes.iter().collect::<BTreeSet<_>>().len(), 1, "{shapes:?}");
}

#[tokio::test]
async fn postgres_consignment_rights_every_port_notice_kind_is_accepted_by_the_ddl() {
    let rig = Rig::new().await;
    let definition: String = sqlx::query_scalar(
        "SELECT pg_get_constraintdef(oid) FROM pg_constraint WHERE conname = 'c7_notice_kind_ck'",
    )
    .fetch_one(&rig.pool)
    .await
    .expect("c7_notice_kind_ck");
    for kind in RightNoticeKind::ALL {
        assert!(definition.contains(&format!("'{}'", kind.as_str())), "{kind:?}: {definition}");
    }
}
