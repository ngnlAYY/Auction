//! auction 上下文。

pub(crate) mod competition;
pub(crate) mod lifecycle;

pub use competition::{
    CREATE_PROXY_COMMAND_KIND, CompetitionService, CreateProxyCommand, LOWER_PROXY_COMMAND_KIND,
    LowerProxyCommand, PLACE_BID_COMMAND_KIND, PlaceBidCommand,
};
pub use lifecycle::AuctionLifecycleService;
