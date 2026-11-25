//! 购前与首次派发前的只读资金预检。

use auction_domain::SettlementWorld;
use auction_domain::consignment::funding::FundingMode;
use auction_time::Timestamp;
use uuid::Uuid;

use crate::ports::settlement::FundingFuture;

/// 预检结论；买后条件变化只保留权利、等待原 UID 核对。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FundingPreflightFact {
    Ready {
        uid: String,
        mode: FundingMode,
        boundary_at: Timestamp,
        boundary_source_id: Option<Uuid>,
        account_version: i64,
    },
    Unavailable(FundingUnavailable),
}

/// 预检不可用的原因闭集。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FundingUnavailable {
    AccountMissing,
    NotReady,
    Frozen,
    ModeUnknown,
    BoundaryUnproven,
    OpeningBlocked,
    GlobalRecordOnlyConflict,
    EvidenceCapabilityMissing,
}

/// 只读预检：不取资金锁；认证 Bot UID 由装配注入，不由调用方传入。
pub trait ConsignmentFundingPreflight: Send + Sync {
    fn preflight(&self, world: SettlementWorld) -> FundingFuture<'_, FundingPreflightFact>;
}
