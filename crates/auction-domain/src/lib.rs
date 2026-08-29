//! Auction 的纯领域模型、值对象和不变量。

mod auction;
mod bidding;
mod equipment;
mod errors;
mod identity;
mod proxy;
mod settlement;

pub use auction::{Auction, AuctionStatus};
pub use bidding::{
    AcceptedBid, AuctionRules, BidAmount, BidDecision, BidMechanism, CREDIT_GRID, CompetitionPlan,
    CompetitionTrigger, CompetitionTriggerKind, DEFAULT_BID_INCREMENT_PPM, DEFAULT_CANCEL_CUTOFF,
    DEFAULT_FINAL_PHASE_LEAD, DEFAULT_PROXY_MAX_OUTBID_USES, DEFAULT_PROXY_SLOW_INTERVAL,
    DEFAULT_SOFT_CLOSE_EXTENSION, DEFAULT_SOFT_CLOSE_WINDOW, DEFAULT_START_PRICE,
    LARGE_BID_THRESHOLD, LotBidState, PROXY_LAST_INCREMENT, REGULAR_BID_INCREMENT,
    SMALL_BID_INCREMENT, SMALL_BID_THRESHOLD,
};
pub use equipment::{
    EquipmentKey, EquipmentKeyEnvelope, EquipmentKeyEnvelopeError, EquipmentKeyError,
};
pub use errors::DomainError;
pub use identity::{
    AuctionId, BidId, CommandId, EquipmentId, LotId, ProxyAuthorizationId, SettlementId,
    SubmissionId, UserId, WorldId, deterministic_uuid, legacy_natural_key_uuid,
};
pub use proxy::{
    ProxyAuthorization, ProxyMode, ProxyStatus, ProxyTerminalReason, ProxyTransition,
    ProxyTransitionKind,
};
pub use settlement::{
    AdjustmentAnchor, RecoveryMode, RecoveryPolicy, SETTLEMENT_PPM, SettlementAmount,
    SettlementFact, SettlementLine, SettlementMode, SettlementObligation, SettlementObligationKind,
    SettlementObligationState, SettlementPlan, SettlementPolicy, SettlementWorld, floor_ppm,
    proportional_floor,
};
