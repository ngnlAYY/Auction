//! 受限权利端口的内存假实现与测试用密码适配器。
//!
//! 假实现与真实存储遵守同一合同：真实、虚拟与未匹配目标走同一段写路径；命令账本先于预算与
//! 业务判断；挑战失败次数与通知间隔按领域常量计；密码端口未注入时失败关闭。

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use auction_domain::consignment::rights::{
    BUDGET_WINDOW, CHALLENGE_CODE_ENTROPY_BYTES, CHALLENGE_TTL, ChallengeCode, ChallengeState,
    LocatorText, RightActionFacts, RightKind, RightLocator, RightSessionToken, RightTarget,
    RightView, RightsError, SESSION_TTL, SessionDigest, allowed_actions, format_locator,
    locator_digest, notice_allowed, request_within_budget, rotated_session_expiry, session_digest,
};
use auction_domain::{CommandId, SettlementWorld};
use auction_time::Timestamp;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use uuid::Uuid;

use super::{
    BrowserBinding, ChallengeAad, ChallengeIssued, ChallengeRightCommand, ClaimDisposition,
    ClaimOutcome, ClaimRightCommand, LocateRightCommand, RightChallengeCrypto, RightNoticeKind,
    RightNoticeRequest, RightReceipt, RightVerified, RightsStoreError, VerifyRightCommand,
};
use crate::ports::core::RequestHash;
use records::{
    ChallengeRecord, RightRecord, SessionRecord, Stage, State, Stored, pad_key, uuid_from,
};

mod crypto;
mod records;
mod store_impl;

pub(crate) use crypto::FakeChallengeCrypto;

/// 单一内存假实现；`crypto` 为空模拟密码端口未注入。
pub(crate) struct FakeRightsStore {
    state: Mutex<State>,
    crypto: Option<Arc<dyn RightChallengeCrypto>>,
}

/// 冻结身份摘要：世界码与角色名。
pub(crate) fn identity_digest(name: &str, world: SettlementWorld) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(b"identity");
    hash.update([u8::from(matches!(world, SettlementWorld::Isekai))]);
    hash.update(name.as_bytes());
    hash.finalize().into()
}

impl FakeRightsStore {
    pub(crate) fn new(crypto: Option<Arc<dyn RightChallengeCrypto>>) -> Self {
        let clock = Timestamp::from_timestamp(1_800_000_000, 0).unwrap_or_else(Timestamp::now);
        Self {
            state: Mutex::new(State {
                clock,
                counter: 0,
                rights: Vec::new(),
                challenges: HashMap::new(),
                notices: Vec::new(),
                last_notice: HashMap::new(),
                browser_hits: HashMap::new(),
                ip_hits: HashMap::new(),
                sessions: HashMap::new(),
                ledger: HashMap::new(),
                receipts: HashMap::new(),
            }),
            crypto,
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn now(&self) -> Timestamp {
        self.lock().clock
    }

    pub(crate) fn advance(&self, seconds: i64) {
        let mut state = self.lock();
        if let Some(next) = state.clock.checked_add_signed(auction_time::Duration::seconds(seconds))
        {
            state.clock = next;
        }
    }

    /// 种一条权利，返回权利 ID 与明文定位符（测试里代表用户手里的那份）。
    pub(crate) fn seed_right(
        &self,
        identity: &str,
        world: SettlementWorld,
        kind: RightKind,
        facts: RightActionFacts,
    ) -> (Uuid, LocatorText) {
        let mut state = self.lock();
        let right_id = uuid_from(state.entropy(b"right"));
        let entropy = state.entropy(b"locator");
        let text = format_locator(entropy);
        let locator = locator_digest(&RightLocator::from_entropy(entropy));
        let target = RightTarget::Contract(uuid_from(state.entropy(b"target")));
        state.rights.push(RightRecord {
            right_id,
            version: 1,
            kind,
            world,
            identity_digest: identity_digest(identity, world),
            locator,
            target,
            facts,
            stage: Stage::Open,
        });
        (right_id, text)
    }

    pub(crate) fn complete(&self, right_id: Uuid) {
        if let Some(right) = self.lock().rights.iter_mut().find(|right| right.right_id == right_id)
        {
            right.stage = Stage::Completed;
        }
    }

    pub(crate) fn session_count(&self) -> usize {
        self.lock().sessions.len()
    }

    pub(crate) fn notices(&self) -> Vec<RightNoticeRequest> {
        self.lock().notices.clone()
    }

    /// 在传输边界解封最近一封挑战通知里的码。
    pub(crate) fn open_notice_code(&self, notice: &RightNoticeRequest) -> Option<ChallengeCode> {
        let crypto = self.crypto.as_ref()?;
        let state = self.lock();
        let aad = state.challenges.get(&notice.challenge_id?)?.aad;
        crypto.open(notice.sealed.as_ref()?, aad).ok()
    }

    fn crypto(&self) -> Result<&dyn RightChallengeCrypto, RightsStoreError> {
        self.crypto.as_deref().ok_or(RightsStoreError::Unavailable)
    }

    fn replay(
        state: &State,
        command_id: CommandId,
        hash: RequestHash,
    ) -> Result<Option<Stored>, RightsStoreError> {
        match state.ledger.get(&command_id) {
            None => Ok(None),
            Some((stored, _)) if *stored != hash => Err(RightsStoreError::CommandIdConflict),
            Some((_, result)) => Ok(Some(*result)),
        }
    }

    fn spend_budget(
        state: &mut State,
        browser: BrowserBinding,
        ip: IpAddr,
    ) -> Result<(), RightsStoreError> {
        let now = state.clock;
        let in_window = |hit: &Timestamp| now.signed_duration_since(*hit) < BUDGET_WINDOW;
        let browser_hits = state.browser_hits.entry(browser.0).or_default();
        browser_hits.retain(in_window);
        let ip_hits = state.ip_hits.entry(ip).or_default();
        ip_hits.retain(in_window);
        let browser_used = u32::try_from(browser_hits.len()).unwrap_or(u32::MAX);
        let ip_used = u32::try_from(ip_hits.len()).unwrap_or(u32::MAX);
        request_within_budget(browser_used, ip_used)?;
        browser_hits.push(now);
        ip_hits.push(now);
        Ok(())
    }

    /// 找回与发码共用的写路径：真实与虚拟都建挑战记录，只有真实且未被限额才写 outbox。
    fn issue(
        &self,
        state: &mut State,
        kind: RightNoticeKind,
        throttle_key: Option<[u8; 32]>,
        real: Option<(Uuid, u64)>,
        identity_digest: [u8; 32],
        browser: BrowserBinding,
    ) -> Result<ChallengeIssued, RightsStoreError> {
        let crypto = self.crypto()?;
        let now = state.clock;
        let challenge_id = uuid_from(state.entropy(b"challenge"));
        let expires_at =
            now.checked_add_signed(CHALLENGE_TTL).ok_or(RightsStoreError::Unavailable)?;
        let aad = ChallengeAad {
            challenge_id,
            right_id: real.map(|(right_id, _)| right_id),
            identity_digest,
            right_version: real.map_or(0, |(_, version)| version),
        };
        let entropy = state.entropy(b"code");
        let mut code_bytes = [0_u8; CHALLENGE_CODE_ENTROPY_BYTES];
        code_bytes.copy_from_slice(&entropy[..CHALLENGE_CODE_ENTROPY_BYTES]);
        let code = ChallengeCode::from_entropy(code_bytes);
        let mac = crypto.mac(aad, &code)?;
        let sealed = crypto.seal(&code, aad)?;
        let verifiable = matches!(kind, RightNoticeKind::Challenge);
        let last = throttle_key.and_then(|key| state.last_notice.get(&key).copied());
        if real.is_some() && notice_allowed(last, now) {
            let work_id = uuid_from(state.entropy(b"work"));
            state.notices.push(RightNoticeRequest {
                work_id,
                kind,
                identity_digest,
                challenge_id: Some(challenge_id),
                right_id: aad.right_id,
                sealed: verifiable.then_some(sealed),
            });
            if let Some(key) = throttle_key {
                state.last_notice.insert(key, now);
            }
        }
        state.challenges.insert(
            challenge_id,
            ChallengeRecord {
                aad,
                mac,
                verifiable,
                state: ChallengeState { expires_at, failed_attempts: 0, consumed: false },
                browser,
            },
        );
        Ok(ChallengeIssued { challenge_id, expires_at })
    }

    fn do_locate(&self, command: LocateRightCommand) -> Result<ChallengeIssued, RightsStoreError> {
        let mut state = self.lock();
        if let Some(Stored::Issued(issued)) =
            Self::replay(&state, command.command_id, command.request_hash)?
        {
            return Ok(issued);
        }
        Self::spend_budget(&mut state, command.browser, command.client_ip)?;
        let identity = identity_digest(command.game_identity.as_str(), command.world);
        let real = state
            .rights
            .iter()
            .find(|right| right.identity_digest == identity && right.world == command.world)
            .map(|right| (right.right_id, right.version));
        let issued = self.issue(
            &mut state,
            RightNoticeKind::LocatorGuidance,
            Some(identity),
            real,
            identity,
            command.browser,
        )?;
        state.ledger.insert(command.command_id, (command.request_hash, Stored::Issued(issued)));
        Ok(issued)
    }

    fn do_challenge(
        &self,
        command: ChallengeRightCommand,
    ) -> Result<ChallengeIssued, RightsStoreError> {
        let mut state = self.lock();
        if let Some(Stored::Issued(issued)) =
            Self::replay(&state, command.command_id, command.request_hash)?
        {
            return Ok(issued);
        }
        Self::spend_budget(&mut state, command.browser, command.client_ip)?;
        let found = state
            .rights
            .iter()
            .find(|right| right.locator == command.locator_digest)
            .map(|right| (right.right_id, right.version, right.identity_digest));
        let real = found.map(|(right_id, version, _)| (right_id, version));
        let identity = found.map_or([0; 32], |(_, _, identity)| identity);
        let throttle = found.map(|(right_id, _, _)| *right_id.as_bytes()).map(pad_key);
        let issued = self.issue(
            &mut state,
            RightNoticeKind::Challenge,
            throttle,
            real,
            identity,
            command.browser,
        )?;
        state.ledger.insert(command.command_id, (command.request_hash, Stored::Issued(issued)));
        Ok(issued)
    }

    fn do_verify(&self, command: VerifyRightCommand) -> Result<RightVerified, RightsStoreError> {
        let crypto = self.crypto()?;
        let mut state = self.lock();
        let now = state.clock;
        if let Some(Stored::Verified { right_id, version, expires_at, session }) =
            Self::replay(&state, command.command_id, command.request_hash)?
        {
            // 同命令恢复同次授予：只换令牌，沿用原截止。
            let expires_at = rotated_session_expiry(expires_at, now)?;
            let record =
                state.sessions.remove(&session.as_bytes()).ok_or(RightsStoreError::RightExpired)?;
            let token = RightSessionToken::from_entropy(state.entropy(b"session"));
            let digest = session_digest(&token);
            state.sessions.insert(digest.as_bytes(), SessionRecord { expires_at, ..record });
            state.ledger.insert(
                command.command_id,
                (
                    command.request_hash,
                    Stored::Verified { right_id, version, expires_at, session: digest },
                ),
            );
            return Ok(RightVerified {
                right_id,
                right_version: version,
                expires_at,
                session_token: token,
            });
        }
        let invalid = RightsStoreError::ChallengeInvalid;
        let record = state.challenges.get(&command.challenge_id).ok_or(invalid)?;
        record.state.ensure_open(now).map_err(|_: RightsError| invalid)?;
        let (aad, stored_mac, verifiable, browser, challenge_state) =
            (record.aad, record.mac, record.verifiable, record.browser, record.state);
        let current_version = aad
            .right_id
            .and_then(|id| state.rights.iter().find(|right| right.right_id == id))
            .map(|right| right.version);
        let aad =
            ChallengeAad { right_version: current_version.unwrap_or(aad.right_version), ..aad };
        let mac = crypto.mac(aad, &command.code)?;
        let matched = bool::from(mac.ct_eq(&stored_mac))
            && verifiable
            && browser == command.browser
            && current_version.is_some();
        if !matched {
            if let Some(record) = state.challenges.get_mut(&command.challenge_id) {
                record.state = challenge_state.after_failure();
            }
            return Err(invalid);
        }
        let right = aad
            .right_id
            .and_then(|id| state.rights.iter().find(|right| right.right_id == id))
            .ok_or(invalid)?;
        let (right_id, version, actions) =
            (right.right_id, right.version, allowed_actions(right.kind, &right.facts));
        if let Some(record) = state.challenges.get_mut(&command.challenge_id) {
            record.state.consumed = true;
        }
        let expires_at =
            now.checked_add_signed(SESSION_TTL).ok_or(RightsStoreError::Unavailable)?;
        let token = RightSessionToken::from_entropy(state.entropy(b"session"));
        let digest = session_digest(&token);
        state.sessions.insert(
            digest.as_bytes(),
            SessionRecord { right_id, browser: command.browser, expires_at, actions },
        );
        state.ledger.insert(
            command.command_id,
            (
                command.request_hash,
                Stored::Verified { right_id, version, expires_at, session: digest },
            ),
        );
        Ok(RightVerified { right_id, right_version: version, expires_at, session_token: token })
    }

    fn live_session(
        state: &State,
        session: SessionDigest,
        browser: BrowserBinding,
    ) -> Option<&SessionRecord> {
        state
            .sessions
            .get(&session.as_bytes())
            .filter(|record| record.browser == browser && state.clock < record.expires_at)
    }

    fn do_current(&self, session: SessionDigest, browser: BrowserBinding) -> Option<RightView> {
        let state = self.lock();
        let record = Self::live_session(&state, session, browser)?;
        let right = state.rights.iter().find(|right| right.right_id == record.right_id)?;
        let allowed = allowed_actions(right.kind, &right.facts)
            .into_iter()
            .filter(|action| record.actions.contains(action))
            .collect();
        Some(RightView {
            right_id: right.right_id,
            right_version: right.version,
            kind: right.kind,
            world: right.world,
            contract_or_position: right.target,
            allowed_actions: allowed,
        })
    }

    fn do_claim(&self, command: ClaimRightCommand) -> Result<ClaimOutcome, RightsStoreError> {
        let mut state = self.lock();
        let record = Self::live_session(&state, command.session, command.browser)
            .filter(|record| record.right_id == command.right_id)
            .ok_or(RightsStoreError::Unauthenticated)?;
        let (right_id, scoped) = (record.right_id, record.actions.contains(&command.action));
        if let Some(Stored::Claim(outcome)) =
            Self::replay(&state, command.command_id, command.request_hash)?
        {
            return Ok(outcome);
        }
        let right = state
            .rights
            .iter_mut()
            .find(|right| right.right_id == right_id)
            .ok_or(RightsStoreError::RightExpired)?;
        if right.version != command.expected_version {
            return Err(RightsStoreError::StaleVersion);
        }
        if !scoped || !allowed_actions(right.kind, &right.facts).contains(&command.action) {
            return Err(RightsStoreError::ActionNotAllowed);
        }
        let disposition = match right.stage {
            Stage::Completed => ClaimDisposition::AlreadyCompleted,
            Stage::Fulfilling => ClaimDisposition::AlreadyFulfilling,
            Stage::Open => {
                right.stage = Stage::Fulfilling;
                ClaimDisposition::ClaimAccepted
            }
        };
        let outcome = ClaimOutcome { disposition, right_id, right_version: right.version };
        state.ledger.insert(command.command_id, (command.request_hash, Stored::Claim(outcome)));
        state.receipts.insert(
            (right_id, command.command_id),
            RightReceipt { command_id: command.command_id, right_id, disposition },
        );
        Ok(outcome)
    }

    fn do_receipt(
        &self,
        session: SessionDigest,
        browser: BrowserBinding,
        command_id: CommandId,
    ) -> Option<RightReceipt> {
        let state = self.lock();
        let record = Self::live_session(&state, session, browser)?;
        state.receipts.get(&(record.right_id, command_id)).copied()
    }
}
