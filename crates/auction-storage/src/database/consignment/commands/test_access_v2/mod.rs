//! PG18 测试对 v2 账本、锁组与同连接事务的窄入口；不进入默认构建。

mod operations;
mod primitives;
mod queries;
mod receipt;
mod receipt_guards;
mod rejections;
mod scenarios;
mod session_checks;

pub use queries::{
    test_deposit_audit_count, test_deposit_command_exists, test_deposit_event_count,
    test_deposit_listing_state, test_deposit_marker_exists, test_deposit_rejected_audit_count,
};
pub use rejections::TestDepositRejection;

use auction_application::consignment::{
    DEPOSIT_SCHEMA_VERSION, DepositOperation, EnvelopeReceipt, ObservationReceipt,
    OperationsHoldReceipt, ProofReceipt, ReceiptDisposition, RecheckReceipt, ReleaseReceipt,
    ReservationReceipt, ReservationRetired, SeasonCloseReceipt, SeasonNoticeReceipt, StartReceipt,
    WithdrawDepositReceipt,
};
use auction_domain::consignment::{ListingEquipmentIdentity, ListingState, OriginalMailKey};
use serde_json::Value;
use uuid::Uuid;

use super::errors::WriteError;
use super::ledger::{DepositTarget, StoredReceipt};
use super::receipts_v2::DepositReceipt;
use receipt::any;

type TriggerEvent =
    (Uuid, u64, Option<ListingState>, ListingState, &'static str, Option<&'static str>);

/// 测试用锁集合；字段顺序故意允许与规范顺序不同。
#[derive(Debug, Clone, Default)]
pub struct TestDepositLocks {
    /// 命令 UUID。
    pub command_id: Uuid,
    /// 用户锁集合。
    pub sellers: Vec<Uuid>,
    /// 实物锁集合。
    pub equipment: Vec<ListingEquipmentIdentity>,
    /// K 锁集合。
    pub mail_keys: Vec<OriginalMailKey>,
}

/// 测试用的类型化目标，与 `DepositTarget` 一一对应。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestDepositTarget {
    /// 挂单。
    Listing(Uuid),
    /// 预约。
    Reservation(Uuid),
    /// 隔离。
    Quarantine(Uuid),
    /// 最终决定。
    FinalDecision(Uuid),
    /// 原起写 attempt。
    OriginalAttempt(Uuid),
    /// 原信键。
    MailKey(OriginalMailKey),
    /// 冻结季。
    Season {
        /// 季身份。
        season_id: String,
        /// 日历版本摘要。
        calendar_version: [u8; 32],
    },
}

impl From<TestDepositTarget> for DepositTarget {
    fn from(target: TestDepositTarget) -> Self {
        match target {
            TestDepositTarget::Listing(id) => Self::Listing(id),
            TestDepositTarget::Reservation(id) => Self::Reservation(id),
            TestDepositTarget::Quarantine(id) => Self::Quarantine(id),
            TestDepositTarget::FinalDecision(id) => Self::FinalDecision(id),
            TestDepositTarget::OriginalAttempt(id) => Self::OriginalAttempt(id),
            TestDepositTarget::MailKey(key) => Self::MailKey(key),
            TestDepositTarget::Season { season_id, calendar_version } => {
                Self::Season { season_id, calendar_version }
            }
        }
    }
}

/// 首次写入后的账本值。
#[derive(Debug, Clone, PartialEq)]
pub struct TestDepositStoredReceipt {
    /// 首次状态码。
    pub status: i16,
    /// 首次业务码。
    pub code: String,
    /// 首次 JSONB 回执。
    pub body: Value,
}

impl From<StoredReceipt> for TestDepositStoredReceipt {
    fn from(stored: StoredReceipt) -> Self {
        Self { status: stored.status, code: stored.code, body: stored.body }
    }
}

/// 首次写入或同请求重放。
#[derive(Debug, Clone, PartialEq)]
pub enum TestDepositOutcome {
    /// 本事务首次提交。
    First(TestDepositStoredReceipt),
    /// 相同请求的原回执。
    Replay(TestDepositStoredReceipt),
}

/// 4A v2 回执的测试包装；用于逐类型往返。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestDepositReceipt {
    /// 预约签发。
    Reservation(ReservationReceipt),
    /// 预约退休。
    ReservationRetired(ReservationRetired),
    /// 整封决定。
    Envelope(EnvelopeReceipt),
    /// 观察追加。
    Observation(ObservationReceipt),
    /// 隔离重检。
    Recheck(RecheckReceipt),
    /// 隔离解除。
    Release(ReleaseReceipt),
    /// 资产起写。
    Start(StartReceipt),
    /// 证明应用。
    Proof(ProofReceipt),
    /// 撤回意愿或未代管关闭。
    Withdraw(WithdrawDepositReceipt),
    /// 运营止售设置或解除。
    OperationsHold(OperationsHoldReceipt),
    /// 季前通知。
    SeasonNotice(SeasonNoticeReceipt),
    /// 旧季关闭。
    SeasonClose(SeasonCloseReceipt),
}

impl TestDepositReceipt {
    /// 测试回执绑定的命令 UUID。
    pub fn command_id(&self) -> auction_domain::CommandId {
        any(self).command_id()
    }

    fn operation(&self) -> DepositOperation {
        any(self).operation()
    }

    fn status(&self) -> i16 {
        any(self).status()
    }

    /// 4A 回执经真实 `receipts_v2` 序列化后的账本体；跨层对齐测试不得再维护平行线格式。
    pub fn body(&self) -> Value {
        any(self).body()
    }

    fn trigger_event(&self) -> Option<TriggerEvent> {
        match self {
            Self::Reservation(receipt) => Some((
                receipt.listing_id,
                receipt.listing_version.value(),
                Some(ListingState::AwaitingDeposit),
                receipt.state,
                ReceiptDisposition::ReservationIssued.as_str(),
                None,
            )),
            Self::Withdraw(receipt) if receipt.disposition == ReceiptDisposition::Closed => Some((
                receipt.listing_id,
                receipt.listing_version.value(),
                Some(ListingState::AwaitingDeposit),
                receipt.state,
                ReceiptDisposition::Closed.as_str(),
                Some("seller_cancelled"),
            )),
            Self::OperationsHold(receipt) if receipt.disposition == ReceiptDisposition::HoldSet => {
                Some((
                    receipt.listing_id,
                    receipt.listing_version.value(),
                    Some(ListingState::DepositAccepted),
                    receipt.state,
                    ReceiptDisposition::HoldSet.as_str(),
                    Some("operations_hold"),
                ))
            }
            Self::OperationsHold(receipt) if receipt.disposition == ReceiptDisposition::Closed => {
                Some((
                    receipt.listing_id,
                    receipt.listing_version.value(),
                    Some(ListingState::AwaitingDeposit),
                    receipt.state,
                    ReceiptDisposition::Closed.as_str(),
                    Some("admin_delisted"),
                ))
            }
            Self::OperationsHold(receipt)
                if receipt.disposition == ReceiptDisposition::HoldReleased =>
            {
                Some((
                    receipt.listing_id,
                    receipt.listing_version.value(),
                    Some(ListingState::HeldOffMarket),
                    receipt.state,
                    ReceiptDisposition::HoldReleased.as_str(),
                    Some("review"),
                ))
            }
            _ => None,
        }
    }

    fn decode(stored: &StoredReceipt) -> Result<Self, WriteError> {
        match test_operation(stored)? {
            DepositOperation::IssueReservation => {
                Ok(Self::Reservation(ReservationReceipt::decode(stored)?))
            }
            DepositOperation::ExpireReservation => {
                Ok(Self::ReservationRetired(ReservationRetired::decode(stored)?))
            }
            DepositOperation::DecideEnvelope => {
                Ok(Self::Envelope(EnvelopeReceipt::decode(stored)?))
            }
            DepositOperation::AppendQuarantineObservation => {
                Ok(Self::Observation(ObservationReceipt::decode(stored)?))
            }
            DepositOperation::RecheckQuarantine => {
                Ok(Self::Recheck(RecheckReceipt::decode(stored)?))
            }
            DepositOperation::ReleaseQuarantine => {
                Ok(Self::Release(ReleaseReceipt::decode(stored)?))
            }
            DepositOperation::StartDispositionWrite => {
                Ok(Self::Start(StartReceipt::decode(stored)?))
            }
            DepositOperation::ApplyMailProof => Ok(Self::Proof(ProofReceipt::decode(stored)?)),
            DepositOperation::Withdraw => {
                Ok(Self::Withdraw(WithdrawDepositReceipt::decode(stored)?))
            }
            DepositOperation::AdminDelist | DepositOperation::ReleaseOperationsHold => {
                Ok(Self::OperationsHold(OperationsHoldReceipt::decode(stored)?))
            }
            DepositOperation::SeasonNotice => {
                Ok(Self::SeasonNotice(SeasonNoticeReceipt::decode(stored)?))
            }
            DepositOperation::SeasonClose => {
                Ok(Self::SeasonClose(SeasonCloseReceipt::decode(stored)?))
            }
        }
    }
}

/// 测试逐类型往返入口只需操作码；生产读取不依赖 v1 之外的通用解码。
fn test_operation(stored: &StoredReceipt) -> Result<DepositOperation, WriteError> {
    if stored.body.get("schema_version").and_then(Value::as_u64)
        != Some(u64::from(DEPOSIT_SCHEMA_VERSION))
    {
        return Err(WriteError::invariant("consignment_v2_receipt"));
    }
    stored
        .body
        .get("operation")
        .and_then(Value::as_str)
        .and_then(DepositOperation::parse)
        .ok_or_else(|| WriteError::invariant("consignment_v2_receipt"))
}
