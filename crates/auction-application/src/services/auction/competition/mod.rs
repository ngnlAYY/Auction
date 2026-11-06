//! 竞价上下文用例：命令载荷、用例服务、账本重放与端口适配。

mod commands;
mod legacy_ledger;
mod ports;
mod replay;
mod service;
mod slow_replay;

use commands::CompetitionCommand;

pub use commands::{
    CREATE_PROXY_COMMAND_KIND, CreateProxyCommand, LOWER_PROXY_COMMAND_KIND, LowerProxyCommand,
    PLACE_BID_COMMAND_KIND, PlaceBidCommand,
};
pub use service::CompetitionService;
