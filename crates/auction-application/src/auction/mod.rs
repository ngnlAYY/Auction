//! auction 上下文公开 API。

pub use crate::ports::auction::{
    ApplicationFuture, AuctionFinalProxyPlan, AuctionLifecycleAction, AuctionLifecycleActor,
    AuctionLifecycleActorType, AuctionLifecycleCommand, AuctionLifecycleCommandPort,
    AuctionLifecycleContext, AuctionLifecycleEffect, AuctionLifecyclePlan, AuctionLifecyclePolicy,
    AuctionLifecycleResult, AuctionLifecycleSnapshot, AuctionLifecycleTransaction,
    AuctionLifecycleWriteSummary, AuctionPolicyTemplate, AuctionPreviewSuccessorPlan,
    AuctionReader, CompetitionCommandOutcome, CompetitionCommandPort,
    CompetitionProxyCommandOutcome, CompetitionProxyTransitionOutcome, CompetitionWireTimestamp,
    CurrentAuctionSnapshot, DEFAULT_PUBLIC_ACCOUNT_PAGE_SIZE, ExecuteSlowProxyCommand,
    MAX_PUBLIC_ACCOUNT_OFFSET, MAX_PUBLIC_ACCOUNT_PAGE_SIZE, MAX_PUBLIC_BID_LOG_OFFSET,
    MAX_PUBLIC_HISTORY_NAME_TERMS, MAX_PUBLIC_LOT_OFFSET, MAX_PUBLIC_PRICE_HISTORY_OFFSET,
    POLICY_MINIMUM_INCREMENT, PUBLIC_LOT_CATEGORIES, PublicAccountPageRequest, PublicAccountReader,
    PublicAuctionCountdown, PublicAuctionCursor, PublicAuctionListRequest, PublicAuctionPage,
    PublicAuctionProjection, PublicAuctionSummary, PublicBidLogCursor, PublicBidLogEntry,
    PublicBidLogListRequest, PublicBidLogPage, PublicBidLogReader, PublicEquipment,
    PublicEquipmentStat, PublicFinalRestriction, PublicLiveLotDetail, PublicLiveLotSummary,
    PublicLotDetail, PublicLotListRequest, PublicLotPage, PublicLotReader, PublicMyBid,
    PublicMyBidsPage, PublicMyBidsRequest, PublicOwnedProxy, PublicPreviewLotDetail,
    PublicPreviewLotSummary, PublicPriceHistoryCursor, PublicPriceHistoryEntry,
    PublicPriceHistoryListRequest, PublicPriceHistoryPage, PublicPriceHistoryReader,
    PublicPriceHistorySort, PublicPriceHistoryStats, PublicProxyBid, PublicProxyCapabilities,
    PublicProxyTransition, PublicRealtimeAuctionStatus, SLOW_PROXY_COMMAND_KIND,
    SlowProxyCommandPort, SlowProxyDue, SlowProxyDueReader, WorldReader, WorldSnapshot,
    format_public_time_wire, is_stale_slow_proxy_error,
};
pub use crate::services::auction::{
    AuctionLifecycleService, AuctionService, CREATE_PROXY_COMMAND_KIND, CompetitionService,
    CreateAuctionCommand, CreateProxyCommand, LOWER_PROXY_COMMAND_KIND, LowerProxyCommand,
    PLACE_BID_COMMAND_KIND, PlaceBidCommand,
};
