//! auction 上下文。

pub(crate) mod commands;
pub(crate) mod lifecycle;
pub(crate) mod policy;
pub(crate) mod queries;
pub(crate) mod slow_proxy;

pub use commands::{
    ApplicationFuture, CompetitionCommandOutcome, CompetitionCommandPort,
    CompetitionProxyCommandOutcome, CompetitionProxyTransitionOutcome, CompetitionWireTimestamp,
    format_public_time_wire,
};
pub use lifecycle::{
    AuctionFinalProxyPlan, AuctionLifecycleAction, AuctionLifecycleActor,
    AuctionLifecycleActorType, AuctionLifecycleCommand, AuctionLifecycleCommandPort,
    AuctionLifecycleContext, AuctionLifecycleEffect, AuctionLifecyclePlan, AuctionLifecyclePolicy,
    AuctionLifecycleResult, AuctionLifecycleSnapshot, AuctionLifecycleTransaction,
    AuctionLifecycleWriteSummary, AuctionPreviewSuccessorPlan,
};
pub use policy::{AuctionPolicyTemplate, POLICY_MINIMUM_INCREMENT};
pub use queries::{
    AuctionReader, CurrentAuctionSnapshot, DEFAULT_PUBLIC_ACCOUNT_PAGE_SIZE,
    MAX_PUBLIC_ACCOUNT_OFFSET, MAX_PUBLIC_ACCOUNT_PAGE_SIZE, MAX_PUBLIC_BID_LOG_OFFSET,
    MAX_PUBLIC_HISTORY_NAME_TERMS, MAX_PUBLIC_LOT_OFFSET, MAX_PUBLIC_PRICE_HISTORY_OFFSET,
    PUBLIC_LOT_CATEGORIES, PublicAccountPageRequest, PublicAccountReader, PublicAuctionCountdown,
    PublicAuctionCursor, PublicAuctionListRequest, PublicAuctionPage, PublicAuctionProjection,
    PublicAuctionSummary, PublicBidLogCursor, PublicBidLogEntry, PublicBidLogListRequest,
    PublicBidLogPage, PublicBidLogReader, PublicEquipment, PublicEquipmentStat,
    PublicFinalRestriction, PublicLiveLotDetail, PublicLiveLotSummary, PublicLotDetail,
    PublicLotListRequest, PublicLotPage, PublicLotReader, PublicMyBid, PublicMyBidsPage,
    PublicMyBidsRequest, PublicOwnedProxy, PublicPreviewLotDetail, PublicPreviewLotSummary,
    PublicPriceHistoryCursor, PublicPriceHistoryEntry, PublicPriceHistoryListRequest,
    PublicPriceHistoryPage, PublicPriceHistoryReader, PublicPriceHistorySort,
    PublicPriceHistoryStats, PublicProxyBid, PublicProxyCapabilities, PublicProxyTransition,
    PublicRealtimeAuctionStatus, WorldReader, WorldSnapshot,
};
pub use slow_proxy::{
    ExecuteSlowProxyCommand, SLOW_PROXY_COMMAND_KIND, SlowProxyCommandPort, SlowProxyDue,
    SlowProxyDueReader, is_stale_slow_proxy_error,
};
