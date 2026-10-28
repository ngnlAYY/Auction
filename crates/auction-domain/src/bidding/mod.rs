mod amount;
mod bid;
mod competition;
mod policy;

pub use amount::{BidAmount, CREDIT_GRID, MAX_BID_AMOUNT};
pub use bid::{AcceptedBid, BidMechanism};
pub use competition::{CompetitionPlan, CompetitionTrigger, CompetitionTriggerKind};
pub use policy::{
    AuctionRules, DEFAULT_BID_INCREMENT_PPM, DEFAULT_CANCEL_CUTOFF, DEFAULT_FINAL_PHASE_LEAD,
    DEFAULT_PROXY_MAX_OUTBID_USES, DEFAULT_PROXY_SLOW_INTERVAL, DEFAULT_SOFT_CLOSE_EXTENSION,
    DEFAULT_SOFT_CLOSE_WINDOW, DEFAULT_START_PRICE, LotBidState, PROXY_LAST_INCREMENT,
};
