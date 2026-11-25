//! 受限权利用例核心规则：同形、失败关闭、限额、会话与幂等。

use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;

use auction_domain::SettlementWorld;
use auction_domain::consignment::rights::{
    ChallengeCode, RightAction, RightActionFacts, RightKind, format_challenge_code, format_locator,
    format_session_token,
};
use uuid::Uuid;

use super::{
    ChallengeRightInput, ClaimRightInput, LocateRightInput, RightsService, VerifyRightInput,
};

mod golden;
mod hardening;
use crate::ports::consignment::rights::test_support::{FakeChallengeCrypto, FakeRightsStore};
use crate::ports::consignment::{
    BrowserBinding, ClaimDisposition, RightChallengeCrypto, RightsStoreError,
};

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    let mut context = std::task::Context::from_waker(std::task::Waker::noop());
    let mut future = Box::pin(future);
    loop {
        if let std::task::Poll::Ready(value) = future.as_mut().poll(&mut context) {
            return value;
        }
    }
}

fn cmd(seed: u8) -> Uuid {
    uuid::Builder::from_random_bytes([seed; 16]).into_uuid()
}

fn browser(seed: u8) -> BrowserBinding {
    BrowserBinding([seed; 32])
}

fn ip(last: u8) -> IpAddr {
    IpAddr::V4(Ipv4Addr::new(10, 0, 0, last))
}

fn facts() -> RightActionFacts {
    RightActionFacts {
        active: true,
        continue_order: true,
        claim_equipment: true,
        claim_payment: true,
        withdraw_held: true,
    }
}

struct Harness {
    service: RightsService,
    store: Arc<FakeRightsStore>,
}

fn harness() -> Harness {
    let crypto: Arc<dyn RightChallengeCrypto> = Arc::new(FakeChallengeCrypto::new());
    let store = Arc::new(FakeRightsStore::new(Some(crypto.clone())));
    Harness { service: RightsService::new(store.clone(), Some(crypto)), store }
}

impl Harness {
    fn challenge(
        &self,
        seed: u8,
        locator: &str,
    ) -> Result<crate::ports::consignment::ChallengeIssued, RightsStoreError> {
        block_on(self.service.challenge(ChallengeRightInput {
            command_id: cmd(seed),
            browser: browser(1),
            client_ip: ip(1),
            right_locator: locator.to_owned(),
        }))
    }

    fn verify(
        &self,
        seed: u8,
        challenge_id: Uuid,
        code: &ChallengeCode,
    ) -> Result<crate::ports::consignment::RightVerified, RightsStoreError> {
        block_on(self.service.verify(VerifyRightInput {
            command_id: cmd(seed),
            browser: browser(1),
            challenge_id,
            code: format_challenge_code(code).as_str().to_owned(),
        }))
    }

    /// 真实权利走完 发码 → 取码（传输边界解封）→ 核验，返回会话令牌文本与权利版本。
    fn login(&self, seed: u8, locator: &str) -> (String, crate::ports::consignment::RightVerified) {
        let issued = self.challenge(seed, locator).expect("发码");
        let notice = self.store.notices().into_iter().last().expect("通知");
        assert_eq!(notice.challenge_id, Some(issued.challenge_id));
        let code = self.store.open_notice_code(&notice).expect("解封");
        let verified =
            self.verify(seed.wrapping_add(100), issued.challenge_id, &code).expect("核验");
        (format_session_token(&verified.session_token).as_str().to_owned(), verified)
    }

    fn claim(
        &self,
        seed: u8,
        token: &str,
        version: u64,
        action: &str,
    ) -> Result<crate::ports::consignment::ClaimOutcome, RightsStoreError> {
        block_on(self.service.claim(ClaimRightInput {
            command_id: cmd(seed),
            browser: browser(1),
            session_token: token.to_owned(),
            expected_version: version,
            action: action.to_owned(),
        }))
    }
}

fn random_locator(seed: u8) -> String {
    format_locator([seed; 32]).as_str().to_owned()
}

fn wrong_code() -> ChallengeCode {
    ChallengeCode::from_entropy([0x11; 16])
}

/// 同形：真实、虚拟、未匹配目标的响应形状相同，只有真实目标写 outbox。
#[test]
fn locate_and_challenge_answer_real_virtual_and_unmatched_with_same_shape() {
    let h = harness();
    let (_, locator) =
        h.store.seed_right("Alice", SettlementWorld::Main, RightKind::ClaimEquipment, facts());
    let locate = |seed: u8, name: &str, world| {
        block_on(h.service.locate(LocateRightInput {
            command_id: cmd(seed),
            browser: browser(seed),
            client_ip: ip(seed),
            game_identity: name.to_owned(),
            world,
        }))
        .expect("找回")
    };
    let now = h.store.now();
    let expected_expiry = now.checked_add_signed(auction_domain_ttl()).expect("截止");
    let real = locate(1, "Alice", SettlementWorld::Main);
    let virtual_name = locate(2, "Nobody", SettlementWorld::Main);
    let other_world = locate(3, "Alice", SettlementWorld::Isekai);
    for issued in [real, virtual_name, other_world] {
        assert_eq!(issued.expires_at, expected_expiry);
        assert_eq!(issued.challenge_id.get_version_num(), 4);
    }
    assert_eq!(h.store.notices().len(), 1, "只有真实目标写 outbox");

    let real = h.challenge(10, locator.as_str()).expect("真实");
    let unknown = h.challenge(11, &random_locator(9)).expect("虚拟");
    assert_eq!(real.expires_at, unknown.expires_at);
    assert_eq!(real.challenge_id.get_version_num(), unknown.challenge_id.get_version_num());
    assert_eq!(h.store.notices().len(), 2, "虚拟目标没有新增 outbox 行");
}

fn auction_domain_ttl() -> auction_time::Duration {
    auction_domain::consignment::rights::CHALLENGE_TTL
}

/// 虚、错、过期统一 challenge_invalid；对照：正确码成功并发 10 分钟会话。
#[test]
fn verify_rejects_wrong_virtual_and_expired_with_one_error_and_accepts_correct_code() {
    let h = harness();
    let (right_id, locator) =
        h.store.seed_right("Alice", SettlementWorld::Main, RightKind::ClaimEquipment, facts());
    let issued = h.challenge(1, locator.as_str()).expect("发码");
    let code = h.store.open_notice_code(&h.store.notices()[0]).expect("解封");
    let virtual_issued = h.challenge(2, &random_locator(7)).expect("虚拟发码");
    assert_eq!(
        h.verify(3, issued.challenge_id, &wrong_code()).map(|_| ()),
        Err(RightsStoreError::ChallengeInvalid)
    );
    assert_eq!(
        h.verify(4, virtual_issued.challenge_id, &code).map(|_| ()),
        Err(RightsStoreError::ChallengeInvalid)
    );
    assert_eq!(
        h.verify(5, Uuid::nil(), &code).map(|_| ()),
        Err(RightsStoreError::ChallengeInvalid)
    );
    let malformed = block_on(h.service.verify(VerifyRightInput {
        command_id: cmd(6),
        browser: browser(1),
        challenge_id: issued.challenge_id,
        code: "not-a-code".to_owned(),
    }));
    assert_eq!(malformed.map(|_| ()), Err(RightsStoreError::ChallengeInvalid));
    let verified = h.verify(7, issued.challenge_id, &code).expect("正确码成功");
    assert_eq!(verified.right_id, right_id);
    assert_eq!(
        verified.expires_at,
        h.store.now().checked_add_signed(auction_domain_ttl()).expect("截止")
    );

    let expired = h.challenge(8, locator.as_str()).expect("再发");
    assert_eq!(h.store.notices().len(), 1, "10 分钟内不再发信");
    h.store.advance(600);
    assert_eq!(
        h.verify(9, expired.challenge_id, &code).map(|_| ()),
        Err(RightsStoreError::ChallengeInvalid)
    );
}

/// 第 5 次失败后，即使码正确也拒绝；对照：4 次失败后正确码仍成功。
#[test]
fn fifth_wrong_attempt_closes_challenge_but_four_do_not() {
    let h = harness();
    let (_, locator) =
        h.store.seed_right("Alice", SettlementWorld::Main, RightKind::ClaimEquipment, facts());
    let issued = h.challenge(1, locator.as_str()).expect("发码");
    let code = h.store.open_notice_code(&h.store.notices()[0]).expect("解封");
    for seed in 10..14 {
        assert!(h.verify(seed, issued.challenge_id, &wrong_code()).is_err());
    }
    assert!(h.verify(20, issued.challenge_id, &code).is_ok(), "4 次失败后正确码成功");

    h.store.advance(600);
    let second = h.challenge(2, locator.as_str()).expect("再发");
    let notice = h.store.notices().into_iter().last().expect("第二封");
    let code = h.store.open_notice_code(&notice).expect("解封");
    for seed in 30..35 {
        assert!(h.verify(seed, second.challenge_id, &wrong_code()).is_err());
    }
    assert_eq!(
        h.verify(40, second.challenge_id, &code).map(|_| ()),
        Err(RightsStoreError::ChallengeInvalid)
    );
}

/// 每权利每 10 分钟至多一封；响应仍同形。
#[test]
fn at_most_one_notice_per_right_per_ten_minutes() {
    let h = harness();
    let (_, locator) =
        h.store.seed_right("Alice", SettlementWorld::Main, RightKind::ClaimEquipment, facts());
    let first = h.challenge(1, locator.as_str()).expect("第一次");
    let second = h.challenge(2, locator.as_str()).expect("第二次");
    assert_eq!(h.store.notices().len(), 1);
    assert_ne!(first.challenge_id, second.challenge_id);
    h.store.advance(599);
    h.challenge(3, locator.as_str()).expect("窗口内");
    assert_eq!(h.store.notices().len(), 1);
    h.store.advance(1);
    h.challenge(4, locator.as_str()).expect("窗口后");
    assert_eq!(h.store.notices().len(), 2);
}

/// 浏览器 10 次、IP 30 次的预算，超限 429 且与目标真实性无关；对照：换浏览器 / 过窗口恢复。
#[test]
fn budgets_limit_per_browser_and_per_ip_independent_of_target() {
    let h = harness();
    let (_, locator) =
        h.store.seed_right("Alice", SettlementWorld::Main, RightKind::ClaimEquipment, facts());
    let call = |seed: u8, browser_seed: u8, ip_seed: u8, text: &str| {
        block_on(h.service.challenge(ChallengeRightInput {
            command_id: cmd(seed),
            browser: browser(browser_seed),
            client_ip: ip(ip_seed),
            right_locator: text.to_owned(),
        }))
    };
    for seed in 1..=10 {
        call(seed, 1, 1, locator.as_str()).expect("预算内");
    }
    assert_eq!(call(11, 1, 1, locator.as_str()).map(|_| ()), Err(RightsStoreError::BudgetExceeded));
    assert_eq!(
        call(12, 1, 1, &random_locator(3)).map(|_| ()),
        Err(RightsStoreError::BudgetExceeded)
    );
    call(13, 2, 2, locator.as_str()).expect("另一浏览器不受影响");
    h.store.advance(600);
    call(14, 1, 1, locator.as_str()).expect("窗口过后恢复");

    for browser_seed in 20..50 {
        call(100 + browser_seed, browser_seed, 9, &random_locator(5)).expect("IP 预算内");
    }
    assert_eq!(
        call(200, 99, 9, &random_locator(5)).map(|_| ()),
        Err(RightsStoreError::BudgetExceeded)
    );
}

/// 密码端口未注入：发码与核验失败关闭，不写 outbox、不发会话。
#[test]
fn missing_crypto_fails_closed_for_challenge_and_verify() {
    let store = Arc::new(FakeRightsStore::new(None));
    let service = RightsService::new(store.clone(), None);
    let (_, locator) =
        store.seed_right("Alice", SettlementWorld::Main, RightKind::ClaimEquipment, facts());
    let challenge = |seed: u8, text: &str| {
        block_on(service.challenge(ChallengeRightInput {
            command_id: cmd(seed),
            browser: browser(1),
            client_ip: ip(1),
            right_locator: text.to_owned(),
        }))
    };
    assert_eq!(challenge(1, locator.as_str()).map(|_| ()), Err(RightsStoreError::Unavailable));
    assert_eq!(challenge(2, &random_locator(4)).map(|_| ()), Err(RightsStoreError::Unavailable));
    assert!(store.notices().is_empty());
    let verify = block_on(service.verify(VerifyRightInput {
        command_id: cmd(3),
        browser: browser(1),
        challenge_id: Uuid::nil(),
        code: format_challenge_code(&wrong_code()).as_str().to_owned(),
    }));
    assert_eq!(verify.map(|_| ()), Err(RightsStoreError::Unavailable));
}

/// 同命令重放轮换令牌、旧令牌失效、不延寿；换码重放是冲突而不是原会话。
#[test]
fn verify_replay_rotates_token_without_extending_and_rejects_changed_code() {
    let h = harness();
    let (_, locator) =
        h.store.seed_right("Alice", SettlementWorld::Main, RightKind::ClaimEquipment, facts());
    let issued = h.challenge(1, locator.as_str()).expect("发码");
    let code = h.store.open_notice_code(&h.store.notices()[0]).expect("解封");
    let first = h.verify(2, issued.challenge_id, &code).expect("核验");
    let first_text = format_session_token(&first.session_token);
    h.store.advance(300);
    let replay = h.verify(2, issued.challenge_id, &code).expect("同命令重放");
    let replay_text = format_session_token(&replay.session_token);
    assert_ne!(first_text, replay_text);
    assert_eq!(replay.expires_at, first.expires_at, "不延寿");
    assert!(
        block_on(h.service.current(first_text.as_str(), browser(1))).expect("读").is_none(),
        "旧令牌失效"
    );
    assert!(block_on(h.service.current(replay_text.as_str(), browser(1))).expect("读").is_some());
    assert_eq!(
        h.verify(2, issued.challenge_id, &wrong_code()).map(|_| ()),
        Err(RightsStoreError::CommandIdConflict)
    );
    h.store.advance(300);
    assert_eq!(
        h.verify(2, issued.challenge_id, &code).map(|_| ()),
        Err(RightsStoreError::RightExpired)
    );
}

/// 浏览器绑定：核验、读取与续办都必须是原浏览器。
#[test]
fn browser_binding_is_enforced_for_verify_current_and_claim() {
    let h = harness();
    let (_, locator) =
        h.store.seed_right("Alice", SettlementWorld::Main, RightKind::ClaimEquipment, facts());
    let issued = h.challenge(1, locator.as_str()).expect("发码");
    let code = h.store.open_notice_code(&h.store.notices()[0]).expect("解封");
    let stolen = block_on(h.service.verify(VerifyRightInput {
        command_id: cmd(2),
        browser: browser(2),
        challenge_id: issued.challenge_id,
        code: format_challenge_code(&code).as_str().to_owned(),
    }));
    assert_eq!(stolen.map(|_| ()), Err(RightsStoreError::ChallengeInvalid));
    let verified = h.verify(3, issued.challenge_id, &code).expect("原浏览器成功");
    let token = format_session_token(&verified.session_token);
    assert!(block_on(h.service.current(token.as_str(), browser(2))).expect("读").is_none());
    assert!(block_on(h.service.current(token.as_str(), browser(1))).expect("读").is_some());
    let foreign = block_on(h.service.claim(ClaimRightInput {
        command_id: cmd(4),
        browser: browser(2),
        session_token: token.as_str().to_owned(),
        expected_version: 1,
        action: "claim_equipment".to_owned(),
    }));
    assert_eq!(foreign.map(|_| ()), Err(RightsStoreError::Unauthenticated));
}

/// 续办：版本、动作范围与命令幂等各一正一反；损失权利只读。
#[test]
fn claim_enforces_version_scope_and_command_idempotency() {
    let h = harness();
    let (right_id, locator) =
        h.store.seed_right("Alice", SettlementWorld::Main, RightKind::ClaimEquipment, facts());
    let (token, verified) = h.login(1, locator.as_str());
    let view = block_on(h.service.current(&token, browser(1))).expect("读").expect("视图");
    assert_eq!(view.allowed_actions, vec![RightAction::ClaimEquipment]);

    assert_eq!(
        h.claim(10, &token, verified.right_version + 1, "claim_equipment").map(|_| ()),
        Err(RightsStoreError::StaleVersion)
    );
    assert_eq!(
        h.claim(11, &token, verified.right_version, "claim_payment").map(|_| ()),
        Err(RightsStoreError::ActionNotAllowed)
    );
    assert_eq!(
        h.claim(12, &token, verified.right_version, "relist").map(|_| ()),
        Err(RightsStoreError::ActionNotAllowed)
    );
    let accepted = h.claim(13, &token, verified.right_version, "claim_equipment").expect("受理");
    assert_eq!(accepted.disposition, ClaimDisposition::ClaimAccepted);
    assert_eq!(accepted.right_id, right_id);
    assert_eq!(
        h.claim(13, &token, verified.right_version, "claim_equipment"),
        Ok(accepted),
        "同命令同请求返回原结果"
    );
    let other = h.claim(14, &token, verified.right_version, "claim_equipment").expect("另一命令");
    assert_eq!(other.disposition, ClaimDisposition::AlreadyFulfilling);
    assert_eq!(
        h.claim(13, &token, verified.right_version, "claim_payment").map(|_| ()),
        Err(RightsStoreError::CommandIdConflict)
    );
    h.store.complete(right_id);
    let done = h.claim(15, &token, verified.right_version, "claim_equipment").expect("已完成");
    assert_eq!(done.disposition, ClaimDisposition::AlreadyCompleted);

    let (_, loss_locator) =
        h.store.seed_right("Bob", SettlementWorld::Main, RightKind::LossCase, facts());
    h.store.advance(600);
    let (loss_token, loss) = h.login(40, loss_locator.as_str());
    for action in ["continue_order", "claim_equipment", "claim_payment", "withdraw_held"] {
        assert_eq!(
            h.claim(50, &loss_token, loss.right_version, action).map(|_| ()),
            Err(RightsStoreError::ActionNotAllowed)
        );
    }
}

/// 会话过期后续办失败关闭；重新验权后能查旧回执。
#[test]
fn expired_session_cannot_claim_and_new_session_reads_old_receipt() {
    let h = harness();
    let (_, locator) =
        h.store.seed_right("Alice", SettlementWorld::Main, RightKind::ClaimPayment, facts());
    let (token, verified) = h.login(1, locator.as_str());
    h.claim(10, &token, verified.right_version, "claim_payment").expect("受理");
    h.store.advance(600);
    assert_eq!(
        h.claim(11, &token, verified.right_version, "claim_payment").map(|_| ()),
        Err(RightsStoreError::Unauthenticated)
    );
    assert!(block_on(h.service.current(&token, browser(1))).expect("读").is_none());
    let (fresh, _) = h.login(20, locator.as_str());
    let receipt = block_on(h.service.command_receipt(&fresh, browser(1), cmd(10)))
        .expect("读")
        .expect("旧回执");
    assert_eq!(receipt.disposition, ClaimDisposition::ClaimAccepted);
    assert!(
        block_on(h.service.command_receipt(&fresh, browser(2), cmd(10))).expect("读").is_none()
    );
}

/// 输入形状：非 v4 命令 ID、坏定位符、带空白的身份一律失败关闭。
#[test]
fn malformed_inputs_fail_closed() {
    let h = harness();
    let (_, locator) =
        h.store.seed_right("Alice", SettlementWorld::Main, RightKind::ClaimEquipment, facts());
    let v1 = Uuid::from_u128(1);
    let bad_command = block_on(h.service.challenge(ChallengeRightInput {
        command_id: v1,
        browser: browser(1),
        client_ip: ip(1),
        right_locator: locator.as_str().to_owned(),
    }));
    assert_eq!(bad_command.map(|_| ()), Err(RightsStoreError::InvalidRequest));
    assert_eq!(h.challenge(1, "R1-short").map(|_| ()), Err(RightsStoreError::InvalidRequest));
    let padded = block_on(h.service.locate(LocateRightInput {
        command_id: cmd(2),
        browser: browser(1),
        client_ip: ip(1),
        game_identity: " Alice".to_owned(),
        world: SettlementWorld::Main,
    }));
    assert_eq!(padded.map(|_| ()), Err(RightsStoreError::InvalidRequest));
    assert!(h.store.notices().is_empty());
}

/// 泄漏对照：派生 Debug 的同形输入会把秘密原样打出，证明下面的断言能失败。
#[derive(Debug)]
#[allow(dead_code, reason = "只用于 Debug 输出对照，字段本身不被读取")]
struct LeakyInput {
    right_locator: String,
    session_token: String,
}

/// 安全边界：码、定位符与令牌不进 Debug；对照：派生 Debug 的同形结构会泄漏，显式取文本才可见。
#[test]
fn secrets_never_appear_in_debug_output() {
    let h = harness();
    let (_, locator) =
        h.store.seed_right("Alice", SettlementWorld::Main, RightKind::ClaimEquipment, facts());
    let issued = h.challenge(1, locator.as_str()).expect("发码");
    let notice = h.store.notices().remove(0);
    let code = h.store.open_notice_code(&notice).expect("解封");
    let code_text = format_challenge_code(&code);
    let input = VerifyRightInput {
        command_id: cmd(2),
        browser: browser(1),
        challenge_id: issued.challenge_id,
        code: code_text.as_str().to_owned(),
    };
    let verified = h.verify(3, issued.challenge_id, &code).expect("核验");
    let token_text = format_session_token(&verified.session_token).as_str().to_owned();
    let challenge_input = ChallengeRightInput {
        command_id: cmd(4),
        browser: browser(1),
        client_ip: ip(1),
        right_locator: locator.as_str().to_owned(),
    };
    let claim_input = ClaimRightInput {
        command_id: cmd(5),
        browser: browser(1),
        session_token: token_text.clone(),
        expected_version: 1,
        action: "claim_equipment".to_owned(),
    };
    let dump = format!(
        "{notice:?}{input:?}{code:?}{verified:?}{challenge_input:?}{claim_input:?}{:?}{:?}{:?}",
        browser(1),
        verified.session_token,
        RightsStoreError::ChallengeInvalid
    );
    let leaky = format!(
        "{:?}",
        LeakyInput {
            right_locator: locator.as_str().to_owned(),
            session_token: token_text.clone(),
        }
    );
    for secret in [code_text.as_str(), locator.as_str(), token_text.as_str()] {
        // 去掉前缀后的随机部分也不得出现。
        assert!(!dump.contains(secret), "Debug 输出含明文");
        assert!(!dump.contains(&secret[3..]), "Debug 输出含明文随机部分");
    }
    assert!(
        leaky.contains(locator.as_str()) && leaky.contains(token_text.as_str()),
        "对照：派生 Debug 会泄漏"
    );
    assert!(input.code.contains(code_text.as_str()), "对照：显式字段才含明文");
}
