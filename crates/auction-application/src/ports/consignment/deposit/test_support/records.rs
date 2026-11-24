//! 内存假实现的记录类型与计数。

use std::collections::BTreeMap;

use auction_domain::CommandId;
use auction_domain::UserId;
use auction_domain::consignment::deposit::{
    EnvelopeDecision, QuarantineReason, ReleaseKind, ReservationState, TokenText,
};
use auction_domain::consignment::{
    FrozenSeason, HeldOffMarketReason, ListingState, OriginalMailKey,
};
use auction_time::Timestamp;
use uuid::Uuid;

use crate::ports::consignment::{
    AdminObservationItem, DepositOperation, EnvelopeReceipt, ObservationReceipt,
    OperationsHoldReceipt, ProofDisposition, ProofReceipt, RecheckReceipt, ReleaseReceipt,
    ReservationReceipt, ReservationRetired, SeasonCloseReceipt, SeasonNoticeReceipt, StartReceipt,
    WithdrawDepositReceipt,
};
use crate::ports::core::RequestHash;

/// 可独立断言的假实现计数。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct FakeCounters {
    pub permits_issued: u64,
    pub posts_recorded: u64,
    pub settlement_owners: u64,
    pub recovery_owners: u64,
}

/// 命令账本中可供同 ID 同请求重放的首次回执。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LedgerReceipt {
    Reservation(ReservationReceipt),
    Retired(ReservationRetired),
    Envelope(EnvelopeReceipt),
    Observation(ObservationReceipt),
    Recheck(RecheckReceipt),
    Release(ReleaseReceipt),
    Start(StartReceipt),
    Proof(ProofReceipt),
    Withdraw(WithdrawDepositReceipt),
    Hold(OperationsHoldReceipt),
    Notice(SeasonNoticeReceipt),
    Close(SeasonCloseReceipt),
}

/// 全局命令账本记录；摘要已覆盖 actor、目标与输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LedgerRecord {
    pub operation: DepositOperation,
    pub request_hash: RequestHash,
    pub receipt: LedgerReceipt,
}

/// 卖家资格与 allowlist。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SellerRecord {
    pub eligible: bool,
    pub allowlisted: bool,
}

/// 假挂单投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ListingRecord {
    pub version: u64,
    pub state: ListingState,
    pub reason: Option<auction_domain::consignment::ListingReason>,
    pub price: i64,
    pub seller_user_id: UserId,
    pub canonical_eid: String,
    pub hold_id: Option<Uuid>,
    pub hold_reason: Option<HeldOffMarketReason>,
    pub withdrawal_intent: Option<Uuid>,
}

/// 假预约记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReservationRecord {
    pub reservation_id: Uuid,
    pub generation: u64,
    pub listing_id: Uuid,
    pub seller_user_id: UserId,
    pub canonical_eid: String,
    pub token: TokenText,
    pub expires_at: Timestamp,
    pub state: ReservationState,
}

/// 首次终局槽。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TerminalSlot {
    pub decision: EnvelopeDecision,
    pub receipt: EnvelopeReceipt,
}

/// 隔离观察的安全投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FakeObservation {
    pub observation_id: Uuid,
    pub remote_observation_id: String,
    pub observed_at: Timestamp,
    pub parser_version: String,
    pub digest: [u8; 32],
    pub credits: Option<i64>,
    pub cod: Option<i64>,
    pub returnable: bool,
    pub takeable: Option<bool>,
    pub items: Vec<AdminObservationItem>,
}

/// 假隔离记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FakeQuarantine {
    pub quarantine_id: Uuid,
    pub mail_key: OriginalMailKey,
    pub version: u64,
    pub reason: QuarantineReason,
    pub active: bool,
    pub observations: Vec<FakeObservation>,
    pub owner: Option<ReleaseKind>,
    pub release_receipt: Option<ReleaseReceipt>,
}

/// 每个工作唯一的成功起写记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WriteRecord {
    pub receipt: StartReceipt,
    pub delivery_id: Uuid,
    pub fence: u64,
}

/// 内存假实现的全部可变状态。
#[derive(Debug, Clone)]
pub(crate) struct FakeState {
    pub clock: Timestamp,
    pub authenticated_uid: Option<String>,
    pub authenticated_world: auction_domain::SettlementWorld,
    pub credential_generation: u64,
    pub allowlisted: bool,
    pub bot_party: bool,
    pub sellers: BTreeMap<UserId, SellerRecord>,
    pub frozen_season: Option<FrozenSeason>,
    pub current_season: Option<FrozenSeason>,
    pub listings: BTreeMap<Uuid, ListingRecord>,
    pub reservations: BTreeMap<Uuid, ReservationRecord>,
    pub notices: BTreeMap<(Uuid, Uuid, String), Uuid>,
    pub claims: BTreeMap<(String, String), Uuid>,
    pub ledger: BTreeMap<CommandId, LedgerRecord>,
    pub terminals: BTreeMap<OriginalMailKey, TerminalSlot>,
    pub quarantines: BTreeMap<Uuid, FakeQuarantine>,
    pub writes: BTreeMap<Uuid, WriteRecord>,
    pub next_id: u128,
    pub permits_issued: u64,
    pub posts_recorded: u64,
    pub settlement_owners: u64,
    pub recovery_owners: u64,
    pub fail_next_commit: bool,
    pub lose_next_response: bool,
    pub next_proof_disposition: ProofDisposition,
}

impl Default for FakeState {
    fn default() -> Self {
        Self {
            clock: Timestamp::UNIX_EPOCH,
            authenticated_uid: Some("42".to_owned()),
            authenticated_world: auction_domain::SettlementWorld::Main,
            credential_generation: 1,
            allowlisted: true,
            bot_party: false,
            sellers: BTreeMap::new(),
            frozen_season: None,
            current_season: None,
            listings: BTreeMap::new(),
            reservations: BTreeMap::new(),
            notices: BTreeMap::new(),
            claims: BTreeMap::new(),
            ledger: BTreeMap::new(),
            terminals: BTreeMap::new(),
            quarantines: BTreeMap::new(),
            writes: BTreeMap::new(),
            next_id: 1,
            permits_issued: 0,
            posts_recorded: 0,
            settlement_owners: 0,
            recovery_owners: 0,
            fail_next_commit: false,
            lose_next_response: false,
            next_proof_disposition: ProofDisposition::EvidenceQuarantined,
        }
    }
}
