//! Auction 的纯领域模型、值对象和不变量。

mod auction;
mod bidding;
mod delivery;
pub mod equipment;
mod errors;
mod identity;
mod keys;
mod proxy;
mod scheduling;
mod settlement;
mod stable_name;
mod text;
mod watch;

pub use watch::{LotLeadership, LotWatchChannel};

pub use auction::{Auction, AuctionStatus};
pub use bidding::{
    AcceptedBid, AuctionRules, BidAmount, BidMechanism, CREDIT_GRID, CompetitionPlan,
    CompetitionTrigger, CompetitionTriggerKind, DEFAULT_BID_INCREMENT_PPM, DEFAULT_CANCEL_CUTOFF,
    DEFAULT_FINAL_PHASE_LEAD, DEFAULT_PROXY_MAX_OUTBID_USES, DEFAULT_PROXY_SLOW_INTERVAL,
    DEFAULT_SOFT_CLOSE_EXTENSION, DEFAULT_SOFT_CLOSE_WINDOW, DEFAULT_START_PRICE, LotBidState,
    MAX_BID_AMOUNT, PROXY_LAST_INCREMENT,
};
pub use delivery::{
    DeliveryAcknowledgementFacts, DeliveryDispositionActorSource, DeliveryDispositionError,
    DeliveryDispositionKind, DeliveryDispositionSource, PREFLIGHT_LOT_WATCH_INVALIDATED,
    PREFLIGHT_RETIREMENT_CODES, validate_acknowledgement, validate_disposition_actor,
    validate_disposition_reason, validate_disposition_reason_code,
};
pub use equipment::{
    EquipmentKey, EquipmentKeyEnvelope, EquipmentKeyEnvelopeError, EquipmentKeyError, valid_key_id,
};
pub use errors::DomainError;
pub use identity::{
    AuctionId, BidId, CommandId, EquipmentId, LotId, ProxyAuthorizationId, SettlementId,
    SubmissionId, UserId, WorldId, deterministic_uuid, legacy_natural_key_uuid,
};
pub use keys::{DataKeyDomain, DataKeyDomainError};
pub use proxy::{
    ProxyAuthorization, ProxyMode, ProxyStatus, ProxyTerminalReason, ProxyTransition,
    ProxyTransitionKind,
};
pub use scheduling::{
    AuctionEarlyBirdCalendar, AuctionEarlyBirdConfig, AuctionEarlyBirdSeason, AuctionEarlyBirdSlot,
    AuctionScheduleCalendar, AuctionScheduleSlot, AuctionScheduleSource, AuctionScheduleTimes,
    AuctionWeeklySchedule,
};
pub use settlement::{
    AdjustmentAnchor, COMPENSATION_REVERSAL_RULE_VERSION, CompensationReversal,
    DEFAULT_COD_RETURN_LIMIT, EquipmentLossAdmission, EquipmentLossBlocker, FundingAccountKey,
    FundingSplit, InternalSettlementAction, MAX_COD_RETURN_LIMIT, RECOVERY_V2_PAYMENT_TIMEOUT,
    RecoveryAdvance, RecoveryAdvanceError, RecoveryAdvanceFacts, RecoveryMode, RecoveryPolicy,
    RecoveryStage, SETTLEMENT_PPM, SettlementAmount, SettlementFact, SettlementLine,
    SettlementMode, SettlementObligation, SettlementObligationKind, SettlementObligationState,
    SettlementPlan, SettlementPolicy, SettlementWorld, equipment_loss_funding, floor_ppm,
    funding_reconciliation_capacity, next_recovery_advance, proportional_floor,
    validate_settlement_price_breakdown, validate_settlement_price_breakdown_for_mode,
};
pub use text::{valid_optional_text, valid_positive_decimal, valid_text, valid_text_allow_empty};
