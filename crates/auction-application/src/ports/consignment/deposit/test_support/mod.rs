//! 寄售第 4 期收件与代管端口的内存假实现。
//!
//! 假实现把全部状态放在一个可变快照里：成功命令写全局命令账本，失败恢复快照；可以在提交后
//! 模拟响应丢失，并分别统计许可、POST 与两个旧 owner 的接管数。

mod control;
mod ports;
mod reads_v2;
mod records;
mod replay;

use std::sync::{Mutex, MutexGuard};

use auction_domain::CommandId;
use auction_domain::UserId;
use auction_domain::consignment::deposit::{
    QuarantineReason, ReleaseKind, ReservationState, TokenText, format_token,
};
use auction_domain::consignment::{ListingState, ListingVersion, OriginalMailKey};
use auction_time::{Duration, Timestamp};
use uuid::Uuid;

use crate::ports::consignment::ConsignmentError;
use crate::ports::consignment::deposit::commands::{DEPOSIT_SCHEMA_VERSION, ReceiptDisposition};
use crate::ports::consignment::deposit::{
    DepositOperation, PostPermit, ProofDisposition, ReservationReceipt,
};
use crate::ports::core::RequestHash;

pub(crate) use records::{
    FakeCounters, FakeObservation, FakeQuarantine, FakeState, LedgerReceipt, LedgerRecord,
    ListingRecord, ReservationRecord, SellerRecord, TerminalSlot, WriteRecord,
};

/// 单一内存假实现；同时实现第 4 期全部端口 trait。
pub(crate) struct FakeDepositStore {
    state: Mutex<FakeState>,
    staged: Mutex<Option<FakeState>>,
}

impl Default for FakeDepositStore {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeDepositStore {
    /// 建立空状态。
    pub(crate) fn new() -> Self {
        Self { state: Mutex::new(FakeState::default()), staged: Mutex::new(None) }
    }

    /// 替换数据库锁后时钟。
    pub(crate) fn set_clock(&self, clock: Timestamp) {
        self.lock().clock = clock;
    }

    /// 推进锁后时钟。
    pub(crate) fn advance_clock(&self, duration: Duration) {
        let mut state = self.lock();
        state.clock = state.clock.checked_add_signed(duration).expect("测试时钟");
    }

    /// 替换认证 UID、世界与凭据代际。
    pub(crate) fn set_authenticated(
        &self,
        uid: Option<&str>,
        world: auction_domain::SettlementWorld,
        credential_generation: u64,
    ) {
        let mut state = self.lock();
        state.authenticated_uid = uid.map(str::to_owned);
        state.authenticated_world = world;
        state.credential_generation = credential_generation;
    }

    /// 设置 allowlist 与 Bot 当事人标记。
    pub(crate) fn set_allowlist(&self, allowlisted: bool) {
        self.lock().allowlisted = allowlisted;
    }

    /// 设置 Bot 当事人标记。
    pub(crate) fn set_bot_party(&self, bot_party: bool) {
        self.lock().bot_party = bot_party;
    }

    /// 设置卖家资格。
    pub(crate) fn set_seller(&self, seller: UserId, eligible: bool, allowlisted: bool) {
        self.lock().sellers.insert(seller, SellerRecord { eligible, allowlisted });
    }

    /// 设置冻结与当前两套季界。
    pub(crate) fn set_seasons(
        &self,
        frozen: Option<auction_domain::consignment::FrozenSeason>,
        current: Option<auction_domain::consignment::FrozenSeason>,
    ) {
        let mut state = self.lock();
        state.frozen_season = frozen;
        state.current_season = current;
    }

    /// 种一条挂单并取得占用。
    pub(crate) fn seed_listing(
        &self,
        listing_id: Uuid,
        seller: UserId,
        eid: &str,
        state: ListingState,
        version: u64,
        price: i64,
    ) {
        self.set_seller(seller, true, true);
        let mut locked = self.lock();
        locked.listings.insert(
            listing_id,
            ListingRecord {
                version,
                state,
                reason: None,
                price,
                seller_user_id: seller,
                canonical_eid: eid.to_owned(),
                hold_id: None,
                hold_reason: None,
                withdrawal_intent: None,
            },
        );
        locked.claims.insert(
            ("main".to_owned(), eid.to_owned()),
            Uuid::from_u128(listing_id.as_u128().saturating_add(1)),
        );
    }

    /// 种一条有效预约并返回令牌。
    pub(crate) fn seed_reservation(
        &self,
        listing_id: Uuid,
        seller: UserId,
        eid: &str,
        expires_at: Timestamp,
    ) -> (Uuid, TokenText) {
        let mut state = self.lock();
        let reservation_id = self.next_id(&mut state);
        let generation = 7;
        let token = format_token([generation as u8; 32]);
        state.reservations.insert(
            reservation_id,
            ReservationRecord {
                reservation_id,
                generation,
                listing_id,
                seller_user_id: seller,
                canonical_eid: eid.to_owned(),
                token: token.clone(),
                expires_at,
                state: ReservationState::Active,
            },
        );
        state.claims.insert(("main".to_owned(), eid.to_owned()), reservation_id);
        (reservation_id, token)
    }

    /// 种一个活动隔离并返回 ID。
    pub(crate) fn seed_quarantine(
        &self,
        mail_key: OriginalMailKey,
        reason: QuarantineReason,
        observation_id: Uuid,
    ) -> Uuid {
        let mut state = self.lock();
        let quarantine_id = self.next_id(&mut state);
        let observed_at = state.clock;
        state.quarantines.insert(
            quarantine_id,
            FakeQuarantine {
                quarantine_id,
                mail_key,
                version: 1,
                reason,
                active: true,
                observations: vec![FakeObservation {
                    observation_id,
                    remote_observation_id: "seed".to_owned(),
                    observed_at,
                    parser_version: "seed".to_owned(),
                    digest: [9; 32],
                    credits: None,
                    cod: None,
                    returnable: true,
                    takeable: None,
                    items: Vec::new(),
                }],
                owner: None,
                release_receipt: None,
            },
        );
        quarantine_id
    }

    /// 模拟旧消费者已接管该隔离。
    pub(crate) fn seed_existing_owner(&self, quarantine_id: Uuid, kind: ReleaseKind) {
        if let Some(quarantine) = self.lock().quarantines.get_mut(&quarantine_id) {
            quarantine.owner = Some(kind);
        }
    }

    /// 改价但保留当前预约码与代。
    pub(crate) fn reprice_listing(&self, listing_id: Uuid, price: i64) {
        if let Some(listing) = self.lock().listings.get_mut(&listing_id) {
            listing.price = price;
            listing.version += 1;
        }
    }

    /// 读取当前有效预约码。
    pub(crate) fn active_token(&self, listing_id: Uuid) -> Option<TokenText> {
        let state = self.lock();
        state
            .reservations
            .values()
            .find(|reservation| {
                reservation.listing_id == listing_id
                    && reservation.state == ReservationState::Active
            })
            .map(|reservation| reservation.token.clone())
    }

    /// 读取状态。
    pub(crate) fn snapshot(&self) -> FakeState {
        self.lock().clone()
    }

    /// 计数。
    pub(crate) fn counters(&self) -> FakeCounters {
        let state = self.lock();
        FakeCounters {
            permits_issued: state.permits_issued,
            posts_recorded: state.posts_recorded,
            settlement_owners: state.settlement_owners,
            recovery_owners: state.recovery_owners,
        }
    }

    /// 下一次成功命令在提交前失败并整体回滚。
    pub(crate) fn fail_next_commit(&self) {
        self.lock().fail_next_commit = true;
    }

    /// 下一次成功命令提交后返回结果未知。
    pub(crate) fn lose_next_response(&self) {
        self.lock().lose_next_response = true;
    }

    /// 设置下一次证明处置。
    pub(crate) fn set_next_proof_disposition(&self, disposition: ProofDisposition) {
        self.lock().next_proof_disposition = disposition;
    }

    /// 记录一次实际 POST；只有许可与工作记录一致才计数。
    pub(crate) fn record_post(&self, permit: PostPermit) -> bool {
        let mut state = self.lock();
        let Some(write) = state.writes.get(&permit.work_id()) else {
            return false;
        };
        if write.delivery_id != permit.delivery_id() || write.fence != permit.fence() {
            return false;
        }
        state.posts_recorded += 1;
        true
    }

    /// 冻结当前状态，供手工回滚。
    pub(crate) fn begin_stage(&self) {
        *self.staged.lock().expect("staged 锁") = Some(self.lock().clone());
    }

    /// 恢复到最近一次 `begin_stage`。
    pub(crate) fn rollback_stage(&self) {
        let Some(snapshot) = self.staged.lock().expect("staged 锁").take() else {
            return;
        };
        *self.lock() = snapshot;
    }

    fn lock(&self) -> MutexGuard<'_, FakeState> {
        self.state.lock().expect("假实现状态锁")
    }

    fn next_id(&self, state: &mut FakeState) -> Uuid {
        let value = state.next_id;
        state.next_id = state.next_id.saturating_add(1);
        Uuid::from_u128(value)
    }

    fn perform<T>(
        &self,
        command_id: CommandId,
        operation: DepositOperation,
        request_hash: RequestHash,
        replay: impl FnOnce(LedgerReceipt) -> Result<T, ConsignmentError>,
        apply: impl FnOnce(&mut FakeState) -> Result<(LedgerReceipt, T), ConsignmentError>,
    ) -> Result<T, ConsignmentError> {
        let mut state = self.lock();
        if let Some(record) = state.ledger.get(&command_id) {
            if record.operation != operation || record.request_hash != request_hash {
                return Err(ConsignmentError::CommandIdConflict);
            }
            return replay(record.receipt.clone());
        }
        let snapshot = state.clone();
        let (receipt, value) = match apply(&mut state) {
            Ok(result) => result,
            Err(error) => {
                *state = snapshot;
                return Err(error);
            }
        };
        if state.fail_next_commit {
            *state = snapshot;
            state.fail_next_commit = false;
            return Err(ConsignmentError::Unavailable);
        }
        state.ledger.insert(command_id, LedgerRecord { operation, request_hash, receipt });
        if state.lose_next_response {
            state.lose_next_response = false;
            return Err(ConsignmentError::OutcomeUnknown);
        }
        Ok(value)
    }
}

fn reservation_receipt(
    command_id: CommandId,
    listing_id: Uuid,
    listing_version: u64,
    state: ListingState,
    reservation_id: Uuid,
    expires_at: Timestamp,
) -> Result<ReservationReceipt, ConsignmentError> {
    Ok(ReservationReceipt {
        schema_version: DEPOSIT_SCHEMA_VERSION,
        operation: DepositOperation::IssueReservation,
        disposition: ReceiptDisposition::ReservationIssued,
        command_id,
        listing_id,
        listing_version: ListingVersion::new(listing_version)?,
        state,
        reservation_id,
        expires_at,
    })
}
