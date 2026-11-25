//! 受限权利返工回归：一次性消费、找回预算、找回与发码的命令幂等。

use auction_domain::SettlementWorld;
use auction_domain::consignment::rights::{RightKind, format_session_token};

use super::{LocateRightInput, block_on, browser, cmd, facts, harness, ip, random_locator};
use crate::ports::consignment::RightsStoreError;

/// 挑战码一次性：核验成功后，新命令 ID 加正确码再核验是 challenge_invalid 且没有新会话。
#[test]
fn challenge_code_is_consumed_by_a_successful_verify() {
    let h = harness();
    let (_, locator) =
        h.store.seed_right("Alice", SettlementWorld::Main, RightKind::ClaimEquipment, facts());
    let issued = h.challenge(1, locator.as_str()).expect("发码");
    let code = h.store.open_notice_code(&h.store.notices()[0]).expect("解封");
    let first = h.verify(2, issued.challenge_id, &code).expect("首次核验");
    assert_eq!(h.store.session_count(), 1);
    assert_eq!(
        h.verify(3, issued.challenge_id, &code).map(|_| ()),
        Err(RightsStoreError::ChallengeInvalid)
    );
    assert_eq!(h.store.session_count(), 1, "没有新会话");
    assert!(first.expires_at > h.store.now());
}

fn locate(
    h: &super::Harness,
    seed: u8,
    browser_seed: u8,
    ip_seed: u8,
    name: &str,
) -> Result<crate::ports::consignment::ChallengeIssued, RightsStoreError> {
    block_on(h.service.locate(LocateRightInput {
        command_id: cmd(seed),
        browser: browser(browser_seed),
        client_ip: ip(ip_seed),
        game_identity: name.to_owned(),
        world: SettlementWorld::Main,
    }))
}

/// 找回同样计入预算：同浏览器第 11 次、同 IP 第 31 次超限；对照：换浏览器 / 过窗口恢复。
#[test]
fn locate_is_limited_per_browser_and_per_ip() {
    let h = harness();
    h.store.seed_right("Alice", SettlementWorld::Main, RightKind::ClaimEquipment, facts());
    for seed in 1..=10 {
        locate(&h, seed, 1, 1, "Alice").expect("预算内");
    }
    assert_eq!(locate(&h, 11, 1, 1, "Alice").map(|_| ()), Err(RightsStoreError::BudgetExceeded));
    assert_eq!(locate(&h, 12, 1, 1, "Nobody").map(|_| ()), Err(RightsStoreError::BudgetExceeded));
    locate(&h, 13, 2, 2, "Alice").expect("另一浏览器不受影响");
    h.store.advance(600);
    locate(&h, 14, 1, 1, "Alice").expect("窗口过后恢复");

    for browser_seed in 20..50 {
        locate(&h, 100 + browser_seed, browser_seed, 9, "Nobody").expect("IP 预算内");
    }
    assert_eq!(locate(&h, 200, 99, 9, "Nobody").map(|_| ()), Err(RightsStoreError::BudgetExceeded));
}

/// 发码与找回的命令幂等：同 ID 同请求回原结果且不再发信；同 ID 换请求是冲突。
#[test]
fn challenge_and_locate_replay_return_the_original_result() {
    let h = harness();
    let (_, locator) =
        h.store.seed_right("Alice", SettlementWorld::Main, RightKind::ClaimEquipment, facts());
    let first = h.challenge(1, locator.as_str()).expect("发码");
    let notices = h.store.notices().len();
    assert_eq!(h.challenge(1, locator.as_str()), Ok(first));
    assert_eq!(h.store.notices().len(), notices, "重放不再发信");
    assert_eq!(
        h.challenge(1, &random_locator(9)).map(|_| ()),
        Err(RightsStoreError::CommandIdConflict)
    );

    h.store.advance(600);
    let located = locate(&h, 2, 1, 1, "Alice").expect("找回");
    let notices = h.store.notices().len();
    assert_eq!(locate(&h, 2, 1, 1, "Alice"), Ok(located));
    assert_eq!(h.store.notices().len(), notices, "重放不再发信");
    assert_eq!(locate(&h, 2, 1, 1, "Bob").map(|_| ()), Err(RightsStoreError::CommandIdConflict));
}

/// 续办摘要不含会话令牌：核验重放轮换令牌后，同一命令 ID 同请求仍返回原结果；
/// 缺少或格式错误的会话一律未认证。
#[test]
fn claim_digest_ignores_the_session_token_and_missing_session_is_unauthenticated() {
    let h = harness();
    let (_, locator) =
        h.store.seed_right("Alice", SettlementWorld::Main, RightKind::ClaimEquipment, facts());
    let issued = h.challenge(1, locator.as_str()).expect("发码");
    let code = h.store.open_notice_code(&h.store.notices()[0]).expect("解封");
    let first = h.verify(2, issued.challenge_id, &code).expect("核验");
    let first_token = format_session_token(&first.session_token).as_str().to_owned();
    let accepted = h.claim(10, &first_token, first.right_version, "claim_equipment").expect("受理");
    let replay = h.verify(2, issued.challenge_id, &code).expect("同命令重放轮换令牌");
    let second_token = format_session_token(&replay.session_token).as_str().to_owned();
    assert_ne!(first_token, second_token);
    assert_eq!(
        h.claim(10, &second_token, first.right_version, "claim_equipment"),
        Ok(accepted),
        "换令牌不改变请求"
    );
    for bad in ["", "not-a-token", "S1-short"] {
        assert_eq!(
            h.claim(11, bad, first.right_version, "claim_equipment").map(|_| ()),
            Err(RightsStoreError::Unauthenticated)
        );
    }
}
