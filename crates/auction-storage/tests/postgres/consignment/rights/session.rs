//! 会话：同命令恢复轮换令牌、旧令牌失效、不延寿；续办命令幂等与动作范围。

use auction_application::consignment::{
    ClaimDisposition, ClaimRightCommand, RightsStore, RightsStoreError,
};
use auction_application::core::RequestHash;
use auction_domain::CommandId;
use auction_domain::consignment::rights::{RightAction, session_digest};
use uuid::Uuid;

use super::fixture::{Rig, SeededRight, browser, challenge_command, verify_command};

const IP: &str = "198.51.100.7";

/// 发码、解封、核验，返回会话令牌与核验命令（供重放）。
async fn verified(
    rig: &Rig,
    right: &SeededRight,
) -> (
    auction_application::consignment::RightVerified,
    auction_application::consignment::VerifyRightCommand,
) {
    let issued =
        rig.store.challenge(challenge_command(Uuid::new_v4(), 7, IP, right)).await.expect("发码");
    let code = rig.open_code(issued.challenge_id).await;
    let command = verify_command(Uuid::new_v4(), 7, issued.challenge_id, code);
    let granted = rig.store.verify(command.clone()).await.expect("核验");
    (granted, command)
}

#[tokio::test]
async fn postgres_consignment_rights_replay_rotates_the_token_without_extending_the_deadline() {
    let rig = Rig::new().await;
    let right = rig.seed_right(1, "claim_payment", &["claim_payment"]).await;
    let (first, command) = verified(&rig, &right).await;
    let old = session_digest(&first.session_token);
    assert!(rig.store.current(old, browser(7)).await.expect("读取").is_some());
    // 正例：5 分钟后同命令重放，换新令牌、旧令牌失效、截止不变（不延寿）。
    rig.advance_minutes(5);
    let replay = rig.store.verify(command.clone()).await.expect("同命令恢复");
    assert_ne!(session_digest(&replay.session_token).as_bytes(), old.as_bytes(), "令牌必须轮换");
    assert_eq!(replay.expires_at, first.expires_at, "轮换不延寿");
    assert!(rig.store.current(old, browser(7)).await.expect("读取").is_none());
    assert!(
        rig.store
            .current(session_digest(&replay.session_token), browser(7))
            .await
            .expect("读取")
            .is_some()
    );
    // 反例：别的浏览器持新令牌读不到；别的浏览器重放同命令冲突。
    assert!(
        rig.store
            .current(session_digest(&replay.session_token), browser(9))
            .await
            .expect("读取")
            .is_none()
    );
    let mut foreign = command.clone();
    foreign.browser = browser(9);
    assert_eq!(rig.store.verify(foreign).await.err(), Some(RightsStoreError::CommandIdConflict));
    // 反例：过了原截止，同命令重放也失败关闭，令牌随之失效。
    rig.advance_minutes(6);
    assert_eq!(rig.store.verify(command).await.err(), Some(RightsStoreError::RightExpired));
    assert!(
        rig.store
            .current(session_digest(&replay.session_token), browser(7))
            .await
            .expect("读取")
            .is_none()
    );
}

#[tokio::test]
async fn postgres_consignment_rights_claim_is_idempotent_and_bounded_by_the_granted_scope() {
    let rig = Rig::new().await;
    let payable = rig.seed_right(1, "claim_payment", &["claim_payment"]).await;
    let (granted, _) = verified(&rig, &payable).await;
    let session = session_digest(&granted.session_token);
    let command = |id: Uuid, action: RightAction, hash: u8| ClaimRightCommand {
        command_id: CommandId::from_uuid(id),
        right_id: payable.right_id,
        session,
        browser: browser(7),
        expected_version: granted.right_version,
        action,
        request_hash: RequestHash::new([hash; 32]),
    };
    let first_id = Uuid::new_v4();
    // 正例：范围内的动作被受理；同命令同请求重放返回原结果，账本只多一行。
    let accepted =
        rig.store.claim(command(first_id, RightAction::ClaimPayment, 1)).await.expect("受理");
    assert_eq!(accepted.disposition, ClaimDisposition::ClaimAccepted);
    let ledger_rows = rig
        .count("SELECT count(*) FROM public.consignment_commands WHERE operation = 'claim_right'")
        .await;
    let replay =
        rig.store.claim(command(first_id, RightAction::ClaimPayment, 1)).await.expect("重放");
    assert_eq!(replay, accepted);
    assert_eq!(
        rig.count(
            "SELECT count(*) FROM public.consignment_commands WHERE operation = 'claim_right'"
        )
        .await,
        ledger_rows
    );
    // 反例：同命令 ID 不同请求冲突；另一命令不会第二次受理；范围外动作与过期版本被拒。
    assert_eq!(
        rig.store.claim(command(first_id, RightAction::ClaimPayment, 2)).await.err(),
        Some(RightsStoreError::CommandIdConflict)
    );
    let second = rig
        .store
        .claim(command(Uuid::new_v4(), RightAction::ClaimPayment, 3))
        .await
        .expect("第二个命令");
    assert_eq!(second.disposition, ClaimDisposition::AlreadyFulfilling);
    assert_eq!(
        rig.store.claim(command(Uuid::new_v4(), RightAction::WithdrawHeld, 4)).await.err(),
        Some(RightsStoreError::ActionNotAllowed)
    );
    let mut stale = command(Uuid::new_v4(), RightAction::ClaimPayment, 5);
    stale.expected_version += 1;
    assert_eq!(rig.store.claim(stale).await.err(), Some(RightsStoreError::StaleVersion));
    // 回执只对本权利可见。
    let receipt = rig
        .store
        .command_receipt(session, browser(7), CommandId::from_uuid(first_id))
        .await
        .expect("查回执")
        .expect("有回执");
    assert_eq!(receipt.disposition, ClaimDisposition::ClaimAccepted);
    assert!(
        rig.store
            .command_receipt(session, browser(9), CommandId::from_uuid(first_id))
            .await
            .expect("查回执")
            .is_none(),
        "别的浏览器查不到"
    );
}
