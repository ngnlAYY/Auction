//! 假实现的内存状态与确定性“随机量”。

use std::collections::HashMap;
use std::net::IpAddr;

use auction_domain::CommandId;
use auction_domain::SettlementWorld;
use auction_domain::consignment::rights::{
    ChallengeState, LocatorDigest, RightAction, RightActionFacts, RightKind, RightTarget,
    SessionDigest,
};
use auction_time::Timestamp;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::ports::consignment::{
    BrowserBinding, ChallengeAad, ChallengeIssued, ClaimOutcome, RightNoticeRequest, RightReceipt,
};
use crate::ports::core::RequestHash;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Stage {
    Open,
    Fulfilling,
    Completed,
}

pub(super) struct RightRecord {
    pub(super) right_id: Uuid,
    pub(super) version: u64,
    pub(super) kind: RightKind,
    pub(super) world: SettlementWorld,
    pub(super) identity_digest: [u8; 32],
    pub(super) locator: LocatorDigest,
    pub(super) target: RightTarget,
    pub(super) facts: RightActionFacts,
    pub(super) stage: Stage,
}

pub(super) struct ChallengeRecord {
    pub(super) aad: ChallengeAad,
    pub(super) mac: [u8; 32],
    pub(super) verifiable: bool,
    pub(super) state: ChallengeState,
    pub(super) browser: BrowserBinding,
}

pub(super) struct SessionRecord {
    pub(super) right_id: Uuid,
    pub(super) browser: BrowserBinding,
    pub(super) expires_at: Timestamp,
    pub(super) actions: Vec<RightAction>,
}

#[derive(Clone, Copy)]
pub(super) enum Stored {
    Issued(ChallengeIssued),
    Verified { right_id: Uuid, version: u64, expires_at: Timestamp, session: SessionDigest },
    Claim(ClaimOutcome),
}

pub(super) struct State {
    pub(super) clock: Timestamp,
    pub(super) counter: u64,
    pub(super) rights: Vec<RightRecord>,
    pub(super) challenges: HashMap<Uuid, ChallengeRecord>,
    pub(super) notices: Vec<RightNoticeRequest>,
    pub(super) last_notice: HashMap<[u8; 32], Timestamp>,
    pub(super) browser_hits: HashMap<[u8; 32], Vec<Timestamp>>,
    pub(super) ip_hits: HashMap<IpAddr, Vec<Timestamp>>,
    pub(super) sessions: HashMap<[u8; 32], SessionRecord>,
    pub(super) ledger: HashMap<CommandId, (RequestHash, Stored)>,
    pub(super) receipts: HashMap<(Uuid, CommandId), RightReceipt>,
}

impl State {
    /// 确定性的“随机量”：计数器加标签的 SHA-256；真实适配器用 CSPRNG。
    pub(super) fn entropy(&mut self, label: &[u8]) -> [u8; 32] {
        self.counter += 1;
        let mut hash = Sha256::new();
        hash.update(label);
        hash.update(self.counter.to_be_bytes());
        hash.finalize().into()
    }
}

pub(super) fn uuid_from(bytes: [u8; 32]) -> Uuid {
    let mut random = [0_u8; 16];
    random.copy_from_slice(&bytes[..16]);
    uuid::Builder::from_random_bytes(random).into_uuid()
}

pub(super) fn pad_key(right: [u8; 16]) -> [u8; 32] {
    let mut key = [0_u8; 32];
    key[..16].copy_from_slice(&right);
    key
}
