//! 只记账账户的独立开户入口；不改变旧 bot-funds 命令或其幂等摘要。

mod args;
#[cfg(feature = "postgres")]
mod execute;
#[cfg(feature = "postgres")]
mod output;

pub(crate) use args::BotFundsRecordOnlyArgs;
#[cfg(feature = "postgres")]
pub(super) use execute::run;

#[cfg(not(feature = "postgres"))]
/// 无数据库 feature 的构建明确拒绝开户命令。
pub(super) async fn run(_args: BotFundsRecordOnlyArgs) -> anyhow::Result<()> {
    anyhow::bail!("bot-funds-record-only 要求 auctionctl postgres feature")
}
