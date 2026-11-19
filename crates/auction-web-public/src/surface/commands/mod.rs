//! Public 竞价命令组件的稳定出口。
//! 表单冻结 UUID 与负载以供确认；身份宿主在发送后拥有命令与请求任务。

mod bid;
#[cfg(test)]
pub(super) mod form_fixture;
mod host;
mod location;
mod notice;
mod pending;
mod proxy;
mod request;
mod retry;
mod session;
#[cfg(test)]
pub(super) mod session_fixture;
#[cfg(test)]
mod tests;

pub(super) use bid::{BidForm, BidFormInput};
pub(super) use host::{
    CompetitionCommandHost, CompetitionCommands, use_competition_commands_provider,
};
pub(super) use location::{CommandAuctionContext, use_command_auction_provider};
pub(super) use pending::{BidConfirmCommand, ProxyConfirmCommand, PublicCommandIdentity};
pub(super) use proxy::{ProxyAuthorizationRead, ProxyForm, ProxyFormInput};
#[cfg(test)]
pub(super) use request::CommandStatus;
pub(super) use request::{CommandKind, CommandPayload, CommandReceipt, CommandTarget};
pub(super) use retry::CommandRecovery;
pub(super) use session::{CommandSession, SessionAuthority, use_command_session_provider};
pub(super) use session::{PublicSessionChecks, use_public_session_checks};
#[cfg(test)]
pub(super) use tests::test_support;
